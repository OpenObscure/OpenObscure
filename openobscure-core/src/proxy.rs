use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri},
};
use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::{BodyExt, BodyStream};
use hyper_util::client::legacy::Client;

use crate::config::{AppConfig, FailMode, ProviderConfig};
use crate::health::HealthStats;
use crate::hybrid_scanner::HybridScanner;
use crate::image_pipeline::ImageModelManager;
use crate::key_manager::KeyManager;
use crate::kws_engine::KwsEngine;
use crate::mapping::MappingStore;
use crate::pii_types::PiiType;
use crate::response_integrity::ResponseIntegrityScanner;
use crate::vault::Vault;

type HttpsConnector =
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>;

/// Headers that must not be forwarded to upstream (hop-by-hop per RFC 7230 §6.1).
///
/// These headers are meaningful only for a single transport hop and must be
/// stripped before relaying. Forwarding them to the upstream LLM provider
/// would corrupt the connection (e.g., `transfer-encoding: chunked` on an
/// already-buffered body) or leak proxy topology (`via`, `proxy-*`).
const HOP_BY_HOP_HEADERS: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "host",
];

/// Shared application state passed to all handlers.
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<AppConfig>,
    pub scanner: Arc<HybridScanner>,
    pub key_manager: Arc<KeyManager>,
    pub mapping_store: MappingStore,
    pub http_client: Client<HttpsConnector, Body>,
    pub vault: Arc<Vault>,
    pub health: HealthStats,
    pub image_models: Option<Arc<ImageModelManager>>,
    pub kws_engine: Option<Arc<KwsEngine>>,
    pub response_integrity: Option<Arc<ResponseIntegrityScanner>>,
    pub device_tier: crate::device_profile::CapabilityTier,
    pub inspect: bool,
    pub request_journal: Option<Arc<crate::crash_buffer::RequestJournal>>,
}

/// The main proxy handler. All requests flow through here.
pub async fn proxy_handler(
    State(state): State<AppState>,
    req: Request<Body>,
) -> Result<Response<Body>, StatusCode> {
    let request_start = std::time::Instant::now();
    let request_id = uuid::Uuid::new_v4();
    let method = req.method().clone();
    let uri = req.uri().clone();
    let original_headers = req.headers().clone();

    state.health.record_request();

    oo_info!(crate::oo_log::modules::PROXY, "Incoming request",
        request_id = %request_id,
        method = %method,
        uri = %uri);

    // 1. Resolve provider by path prefix
    let (provider_name, provider) = resolve_provider(&state.config, &uri).ok_or_else(|| {
        oo_warn!(crate::oo_log::modules::PROXY, "No provider matched", path = %uri.path());
        StatusCode::NOT_FOUND
    })?;

    oo_debug!(crate::oo_log::modules::PROXY, "Matched provider", request_id = %request_id, provider = %provider_name);

    // 2. Buffer the request body (tier-aware limit)
    let effective_limit = state.config.proxy.body_limit_for_tier(state.device_tier);
    let body_bytes = buffer_body(req, effective_limit).await?;

    // Inspect: print incoming text from agent
    if state.inspect && !body_bytes.is_empty() {
        crate::inspect::print_incoming_text(&request_id, &body_bytes);
    }

    // 3. Check if body is scannable (JSON content type or missing content type with body)
    let should_scan = !body_bytes.is_empty()
        && state.config.scanner.enabled
        && state.config.fpe.enabled
        && is_json_content(&original_headers);

    if !body_bytes.is_empty() && !is_json_content(&original_headers) {
        oo_debug!(crate::oo_log::modules::PROXY, "Non-JSON body, passing through without scanning",
            request_id = %request_id,
            content_type = ?original_headers.get(header::CONTENT_TYPE).map(|v| v.to_str().unwrap_or("<invalid>")));
    }

    // 4. Scan and encrypt PII (only for JSON bodies)
    let versioned_engine = state.key_manager.current().await;
    let (modified_body, mappings, body_timing, fpe_unprotected_count) = if should_scan {
        let scan_start = std::time::Instant::now();
        let fetch_config = state.config.image.to_fetch_config();
        match crate::body::process_request_body(
            &body_bytes,
            &request_id,
            &state.scanner,
            &versioned_engine.engine,
            versioned_engine.version,
            &state.config.scanner.skip_fields,
            state.image_models.as_ref(),
            state.kws_engine.as_ref(),
            Some(&state.http_client),
            &fetch_config,
            state.config.proxy.fail_mode,
            state.inspect,
        )
        .await
        {
            Ok(result) => {
                let encrypt_elapsed = scan_start.elapsed();
                state.health.scan_latency.record(encrypt_elapsed);
                oo_info!(crate::oo_log::modules::PROXY, "Encrypt (request scan+FPE)",
                    request_id = %request_id,
                    elapsed_ms = encrypt_elapsed.as_millis() as u64,
                    pii_matches = result.mappings.by_ciphertext.len());
                if !result.mappings.is_empty() {
                    let count = result.mappings.by_ciphertext.len() as u64;
                    state.health.record_pii_matches(count);
                    log_pii_stats(&request_id, &result.mappings);
                }
                // Record per-feature latency histograms
                if result.text_scan_us > 0 {
                    state
                        .health
                        .text_scan_latency
                        .record(std::time::Duration::from_micros(result.text_scan_us));
                }
                if result.fpe_us > 0 {
                    state
                        .health
                        .fpe_latency
                        .record(std::time::Duration::from_micros(result.fpe_us));
                }
                if result.image_us > 0 {
                    state
                        .health
                        .image_latency
                        .record(std::time::Duration::from_micros(result.image_us));
                }
                if result.voice_us > 0 {
                    state
                        .health
                        .voice_latency
                        .record(std::time::Duration::from_micros(result.voice_us));
                }
                // Record per-image stats and per-phase histograms
                for is in &result.image_stats {
                    state.health.record_images_processed(1);
                    state.health.record_faces_redacted(is.faces_redacted as u64);
                    state
                        .health
                        .record_text_regions(is.text_regions_found as u64);
                    if is.nsfw_detected {
                        state.health.record_nsfw_blocked(1);
                    }
                    if is.is_screenshot {
                        state.health.record_screenshots_detected(1);
                    }
                    for _ in 0..is.onnx_panics {
                        state.health.record_onnx_panic();
                    }
                    if is.nsfw_ms > 0 {
                        state
                            .health
                            .nsfw_latency
                            .record(std::time::Duration::from_millis(is.nsfw_ms));
                    }
                    if is.face_ms > 0 {
                        state
                            .health
                            .face_latency
                            .record(std::time::Duration::from_millis(is.face_ms));
                    }
                    if is.ocr_ms > 0 {
                        state
                            .health
                            .ocr_latency
                            .record(std::time::Duration::from_millis(is.ocr_ms));
                    }
                }
                if !result.image_stats.is_empty() {
                    let total_faces: u32 =
                        result.image_stats.iter().map(|s| s.faces_redacted).sum();
                    let total_ocr: u32 = result
                        .image_stats
                        .iter()
                        .map(|s| s.text_regions_found)
                        .sum();
                    let any_nsfw = result.image_stats.iter().any(|s| s.nsfw_detected);
                    oo_info!(crate::oo_log::modules::IMAGE, "Image pipeline processed",
                        request_id = %request_id,
                        images = result.image_stats.len(),
                        faces_redacted = total_faces,
                        text_regions = total_ocr,
                        nsfw_detected = any_nsfw,
                        elapsed_us = result.image_us);
                }
                // Inspect: print redacted text and PII matches
                if state.inspect {
                    crate::inspect::print_redacted_text(
                        &request_id,
                        &result.body,
                        &result.mappings,
                    );
                }
                // Stash timing for response headers
                let body_timing = (
                    result.text_scan_us,
                    result.fpe_us,
                    result.image_us,
                    result.voice_us,
                    result.voice_kws_us,
                    result.image_stats.iter().map(|s| s.nsfw_ms).sum::<u64>(),
                    result.image_stats.iter().map(|s| s.face_ms).sum::<u64>(),
                    result.image_stats.iter().map(|s| s.ocr_ms).sum::<u64>(),
                );
                let fpe_unprotected = result.fpe_unprotected_count;
                if fpe_unprotected > 0 {
                    state.health.record_fpe_unprotected(fpe_unprotected);
                }
                (result.body, result.mappings, body_timing, fpe_unprotected)
            }
            Err(e) => {
                state.health.scan_latency.record(scan_start.elapsed());
                match state.config.proxy.fail_mode {
                    FailMode::Open => {
                        oo_warn!(crate::oo_log::modules::PROXY, "Body processing failed (fail-open), forwarding original",
                            request_id = %request_id,
                            error = %e);
                        (
                            body_bytes.clone(),
                            crate::mapping::RequestMappings::new(request_id),
                            (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64),
                            0u64,
                        )
                    }
                    FailMode::Closed => {
                        oo_error!(crate::oo_log::modules::PROXY, "Body processing failed (fail-closed), rejecting request",
                            request_id = %request_id,
                            error = %e);
                        return Err(StatusCode::BAD_GATEWAY);
                    }
                }
            }
        }
    } else {
        (
            body_bytes,
            crate::mapping::RequestMappings::new(request_id),
            (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64),
            0u64,
        )
    };

    // 4. Store mappings for response decryption
    let has_mappings = !mappings.is_empty();
    if has_mappings {
        state.mapping_store.insert(mappings.clone()).await;
    }

    // 4b. Journal: record in-flight request before forwarding upstream
    if has_mappings {
        if let Some(ref journal) = state.request_journal {
            journal.write_entry(&crate::crash_buffer::JournalEntry {
                request_id,
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
                mapping_count: mappings.by_ciphertext.len() as u32,
                completed: false,
            });
        }
    }

    // 5. Build upstream request (passthrough original headers)
    let upstream_req = build_upstream_request(
        method,
        &uri,
        provider,
        &provider_name,
        &original_headers,
        &modified_body,
        &state.vault,
    )
    .map_err(|e| {
        oo_error!(crate::oo_log::modules::PROXY, "Failed to build upstream request", request_id = %request_id, error = %e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // 6. Forward to upstream
    let upstream_resp = state
        .http_client
        .request(upstream_req)
        .await
        .map_err(|e| {
            oo_error!(crate::oo_log::modules::PROXY, "Upstream request failed", request_id = %request_id, error = %e);
            StatusCode::BAD_GATEWAY
        })?;

    // 7. Check if response is SSE (text/event-stream) for streaming path
    let (parts, resp_body) = upstream_resp.into_parts();
    let is_sse = is_event_stream(&parts.headers);

    // SSE path: FPE ciphertext spans can be split across frames, so we accumulate
    // each `data:` line until a complete token boundary is found before decrypting.
    // Non-SSE responses are fully buffered in step 8 below.
    if is_sse && (has_mappings || state.response_integrity.is_some()) {
        // 7a. SSE streaming path: content-level FPE decryption + RI scanning
        let mappings = state.mapping_store.get(&request_id).await;
        let mapping_count = mappings
            .as_ref()
            .map(|m| m.by_ciphertext.len())
            .unwrap_or(0);
        oo_debug!(crate::oo_log::modules::PROXY, "SSE response streaming",
            request_id = %request_id, mappings = mapping_count);
        let mapping_store = state.mapping_store.clone();
        let journal = state.request_journal.clone();
        let journal_has_mappings = has_mappings;
        let req_id = request_id;
        let sse_flush_timeout =
            std::time::Duration::from_millis(state.config.proxy.sse_flush_timeout_ms);
        let content_decryptor = crate::sse_accumulator::SseContentDecryptor::new();
        let ri_buffer = crate::sse_accumulator::SseRiBuffer::new();
        let ri_scanner = state.response_integrity.clone();
        let ri_min_flags = state.config.response_integrity.ri_min_flags;
        let health = state.health.clone();
        let empty_mappings = crate::mapping::RequestMappings::new(req_id);
        let inspect_mode = state.inspect;

        // Build streaming body that decrypts each chunk at the content level.
        // `done` flag prevents polling after stream end (flush-then-end pattern).
        // `ri_done` flag ensures RI warning is emitted only once after flush.
        let stream = futures_util::stream::unfold(
            (
                resp_body,
                mappings,
                content_decryptor,
                ri_buffer,
                false,
                false,
            ),
            move |(mut body, mappings, mut content_decryptor, mut ri_buffer, done, ri_done)| {
                let mapping_store = mapping_store.clone();
                let journal = journal.clone();
                let req_id = req_id;
                let ri_scanner = ri_scanner.clone();
                let health = health.clone();
                let empty_mappings = empty_mappings.clone();
                async move {
                    if done && ri_done {
                        return None;
                    }

                    // If FPE flush is done but RI warning not yet emitted, emit it
                    if done && !ri_done {
                        let warning = emit_sse_ri_warning_inline(
                            &mut ri_buffer,
                            ri_scanner.as_deref(),
                            ri_min_flags,
                            &req_id,
                            &health,
                        );
                        if !warning.is_empty() {
                            return Some((
                                Ok::<_, std::convert::Infallible>(Bytes::from(warning)),
                                (body, mappings, content_decryptor, ri_buffer, true, true),
                            ));
                        }
                        return None;
                    }

                    let m = mappings.as_ref().unwrap_or(&empty_mappings);

                    match tokio::time::timeout(sse_flush_timeout, body.frame()).await {
                        Ok(Some(Ok(frame))) => {
                            if let Ok(data) = frame.into_data() {
                                // Feed RI buffer with raw SSE data (accumulates text for RI scan)
                                ri_buffer.feed_sse_data(&data);

                                // Check if [DONE] was detected — inject warning BEFORE [DONE]
                                if ri_buffer.has_seen_done() && !ri_done {
                                    // Feed content decryptor (extracts content, skips [DONE])
                                    let chunk = content_decryptor.feed(&data, m);
                                    // Flush remaining buffered content with FPE decryption
                                    let buf_len = content_decryptor.buffer_len();
                                    let flush = content_decryptor.flush(m);
                                    oo_debug!(crate::oo_log::modules::PROXY, "SSE flush",
                                        request_id = %req_id, buffered_chars = buf_len, mappings = m.by_ciphertext.len(), flush_bytes = flush.len());
                                    mapping_store.remove(&req_id).await;
                                    if journal_has_mappings {
                                        if let Some(ref j) = journal {
                                            j.write_entry(&crate::crash_buffer::JournalEntry {
                                                request_id: req_id,
                                                timestamp: std::time::SystemTime::now()
                                                    .duration_since(std::time::UNIX_EPOCH)
                                                    .unwrap_or_default()
                                                    .as_secs()
                                                    as i64,
                                                mapping_count: m.by_ciphertext.len() as u32,
                                                completed: true,
                                            });
                                        }
                                    }

                                    // Inspect: print accumulated SSE response text
                                    if inspect_mode {
                                        crate::inspect::print_sse_response_text(
                                            &req_id,
                                            ri_buffer.text(),
                                        );
                                    }

                                    // Run RI scan and format warning
                                    let warning = emit_sse_ri_warning_inline(
                                        &mut ri_buffer,
                                        ri_scanner.as_deref(),
                                        ri_min_flags,
                                        &req_id,
                                        &health,
                                    );

                                    // Combine: chunk + flush + warning + [DONE]
                                    let mut combined = bytes::BytesMut::new();
                                    combined.extend_from_slice(&chunk);
                                    combined.extend_from_slice(&flush);
                                    combined.extend_from_slice(warning.as_bytes());
                                    combined.extend_from_slice(b"data: [DONE]\n\n");

                                    Some((
                                        Ok::<_, std::convert::Infallible>(combined.freeze()),
                                        (body, mappings, content_decryptor, ri_buffer, false, true),
                                    ))
                                } else {
                                    // Normal frame — feed through content decryptor
                                    let chunk = content_decryptor.feed(&data, m);
                                    Some((
                                        Ok::<_, std::convert::Infallible>(chunk),
                                        (
                                            body,
                                            mappings,
                                            content_decryptor,
                                            ri_buffer,
                                            false,
                                            false,
                                        ),
                                    ))
                                }
                            } else {
                                // Trailers or other frame types — yield empty bytes
                                Some((
                                    Ok(Bytes::new()),
                                    (body, mappings, content_decryptor, ri_buffer, false, false),
                                ))
                            }
                        }
                        Ok(Some(Err(_))) => {
                            // Stream error — flush content decryptor and emit RI warning
                            let flush = content_decryptor.flush(m);
                            mapping_store.remove(&req_id).await;
                            if journal_has_mappings {
                                if let Some(ref j) = journal {
                                    j.write_entry(&crate::crash_buffer::JournalEntry {
                                        request_id: req_id,
                                        timestamp: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs()
                                            as i64,
                                        mapping_count: m.by_ciphertext.len() as u32,
                                        completed: true,
                                    });
                                }
                            }

                            let warning = if !ri_done {
                                emit_sse_ri_warning_inline(
                                    &mut ri_buffer,
                                    ri_scanner.as_deref(),
                                    ri_min_flags,
                                    &req_id,
                                    &health,
                                )
                            } else {
                                String::new()
                            };

                            let mut combined = bytes::BytesMut::new();
                            combined.extend_from_slice(&flush);
                            combined.extend_from_slice(warning.as_bytes());
                            if combined.is_empty() {
                                None
                            } else {
                                Some((
                                    Ok(combined.freeze()),
                                    (body, mappings, content_decryptor, ri_buffer, true, true),
                                ))
                            }
                        }
                        Ok(None) => {
                            // Stream complete — flush remaining buffer and emit RI warning
                            let buf_len = content_decryptor.buffer_len();
                            let flush = content_decryptor.flush(m);
                            if buf_len > 0 {
                                oo_info!(crate::oo_log::modules::PROXY, "SSE stream ended — flushing content buffer",
                                    request_id = %req_id, buffered_chars = buf_len, flush_bytes = flush.len());
                            }
                            mapping_store.remove(&req_id).await;
                            if journal_has_mappings {
                                if let Some(ref j) = journal {
                                    j.write_entry(&crate::crash_buffer::JournalEntry {
                                        request_id: req_id,
                                        timestamp: std::time::SystemTime::now()
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .unwrap_or_default()
                                            .as_secs()
                                            as i64,
                                        mapping_count: m.by_ciphertext.len() as u32,
                                        completed: true,
                                    });
                                }
                            }

                            // Inspect: print accumulated SSE response text
                            if inspect_mode && !ri_done {
                                crate::inspect::print_sse_response_text(&req_id, ri_buffer.text());
                            }

                            let warning = if !ri_done {
                                emit_sse_ri_warning_inline(
                                    &mut ri_buffer,
                                    ri_scanner.as_deref(),
                                    ri_min_flags,
                                    &req_id,
                                    &health,
                                )
                            } else {
                                String::new()
                            };

                            let mut combined = bytes::BytesMut::new();
                            combined.extend_from_slice(&flush);
                            combined.extend_from_slice(warning.as_bytes());
                            if combined.is_empty() {
                                None
                            } else {
                                Some((
                                    Ok(combined.freeze()),
                                    (body, mappings, content_decryptor, ri_buffer, true, true),
                                ))
                            }
                        }
                        Err(_timeout) => {
                            // Timeout between frames — yield empty keepalive.
                            // Do NOT flush content buffer here: premature flush splits
                            // ciphertexts across flush cycles, breaking FPE decryption.
                            // Content is only flushed at [DONE], stream end, or error.
                            Some((
                                Ok(Bytes::new()),
                                (body, mappings, content_decryptor, ri_buffer, false, false),
                            ))
                        }
                    }
                }
            },
        );

        let stream_body = Body::from_stream(stream);
        let mut response = Response::new(stream_body);
        *response.status_mut() = parts.status;
        *response.version_mut() = parts.version;
        // Forward all headers except content-length (streaming has no fixed length)
        for (key, value) in &parts.headers {
            if key == "content-length" {
                continue;
            }
            response.headers_mut().insert(key.clone(), value.clone());
        }

        let total_us = request_start.elapsed().as_micros() as u64;
        state
            .health
            .request_latency
            .record(std::time::Duration::from_micros(total_us));
        inject_timing_headers(response.headers_mut(), body_timing, 0, total_us);
        if fpe_unprotected_count > 0 {
            if let Ok(hv) = HeaderValue::from_str(&fpe_unprotected_count.to_string()) {
                response
                    .headers_mut()
                    .insert("x-openobscure-pii-unprotected", hv);
            }
        }

        oo_debug!(crate::oo_log::modules::PROXY, "SSE response streaming",
            request_id = %request_id,
            status = %response.status());

        return Ok(response);
    }

    // 7b. Non-SSE path: buffer and process response (existing behavior)
    let resp_bytes = resp_body
        .collect()
        .await
        .map_err(|e| {
            oo_error!(crate::oo_log::modules::PROXY, "Failed to read upstream response", request_id = %request_id, error = %e);
            StatusCode::BAD_GATEWAY
        })?
        .to_bytes();

    // 8. Decrypt FPE values in response + detect format for RI
    let resp_content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let decrypt_start = std::time::Instant::now();
    let body_result = if has_mappings {
        if let Some(mappings) = state.mapping_store.get(&request_id).await {
            let result = crate::body::process_response_body(
                &resp_bytes,
                &mappings,
                resp_content_type.as_deref(),
            );
            state.mapping_store.remove(&request_id).await;
            if let Some(ref journal) = state.request_journal {
                journal.write_entry(&crate::crash_buffer::JournalEntry {
                    request_id,
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64,
                    mapping_count: mappings.by_ciphertext.len() as u32,
                    completed: true,
                });
            }
            result
        } else {
            let format = crate::response_format::detect(resp_content_type.as_deref(), &resp_bytes);
            let extracted_text = crate::response_format::extract_text(&resp_bytes, format);
            crate::body::ResponseBodyResult {
                body: resp_bytes,
                extracted_text,
                format,
            }
        }
    } else {
        let format = crate::response_format::detect(resp_content_type.as_deref(), &resp_bytes);
        let extracted_text = crate::response_format::extract_text(&resp_bytes, format);
        crate::body::ResponseBodyResult {
            body: resp_bytes,
            extracted_text,
            format,
        }
    };
    let decrypt_elapsed = decrypt_start.elapsed();
    if has_mappings {
        oo_info!(crate::oo_log::modules::PROXY, "Decrypt (response restore)",
            request_id = %request_id,
            elapsed_us = decrypt_elapsed.as_micros() as u64);
    }

    // 8b. Response integrity scan (cognitive firewall)
    let ri_start = std::time::Instant::now();
    let final_body = if let Some(ref ri_scanner) = state.response_integrity {
        scan_response_integrity(
            &body_result.body,
            body_result.extracted_text.as_deref(),
            body_result.format,
            ri_scanner,
            &request_id,
            &state,
        )
    } else {
        body_result.body
    };
    let ri_us = ri_start.elapsed().as_micros() as u64;
    if ri_us > 0 {
        state
            .health
            .ri_latency
            .record(std::time::Duration::from_micros(ri_us));
    }

    // Inspect: print decrypted response text
    if state.inspect {
        crate::inspect::print_response_text(&request_id, &final_body);
    }

    // 9. Rebuild response with correct content-length
    let mut response = Response::new(Body::from(final_body.clone()));
    *response.status_mut() = parts.status;
    *response.version_mut() = parts.version;
    // Copy response headers, replacing content-length and removing transfer-encoding
    // (we buffer the full body so chunked encoding no longer applies)
    for (key, value) in &parts.headers {
        if key == "content-length" || key == "transfer-encoding" {
            continue;
        }
        response.headers_mut().insert(key.clone(), value.clone());
    }
    response.headers_mut().insert(
        "content-length",
        HeaderValue::from_str(&final_body.len().to_string()).unwrap(),
    );

    let total_us = request_start.elapsed().as_micros() as u64;
    state
        .health
        .request_latency
        .record(std::time::Duration::from_micros(total_us));
    inject_timing_headers(response.headers_mut(), body_timing, ri_us, total_us);
    if fpe_unprotected_count > 0 {
        if let Ok(hv) = HeaderValue::from_str(&fpe_unprotected_count.to_string()) {
            response
                .headers_mut()
                .insert("x-openobscure-pii-unprotected", hv);
        }
    }

    oo_info!(crate::oo_log::modules::PROXY, "Response sent",
        request_id = %request_id,
        status = %response.status());

    Ok(response)
}

/// Find the provider config that matches the request URI path prefix.
pub(crate) fn resolve_provider<'a>(
    config: &'a AppConfig,
    uri: &Uri,
) -> Option<(String, &'a ProviderConfig)> {
    let path = uri.path();
    // Longest-prefix match: sort candidates descending by route_prefix length so
    // `/openrouter/v1` beats `/openrouter` when both prefixes match the path.
    let mut candidates: Vec<_> = config
        .providers
        .iter()
        .filter(|(_, p)| path.starts_with(&p.route_prefix))
        .collect();
    candidates.sort_by(|a, b| b.1.route_prefix.len().cmp(&a.1.route_prefix.len()));
    candidates
        .into_iter()
        .next()
        .map(|(name, provider)| (name.clone(), provider))
}

/// Build the upstream HTTP request.
///
/// Auth strategy (passthrough):
/// Forward all original headers from the host agent (except hop-by-hop and strip_headers).
/// Users share one set of API keys with the host agent — no duplicate key management.
fn build_upstream_request(
    method: Method,
    original_uri: &Uri,
    provider: &ProviderConfig,
    _provider_name: &str,
    original_headers: &HeaderMap,
    body: &Bytes,
    _vault: &Vault,
) -> Result<Request<Body>, String> {
    // Strip the route prefix from the path
    let original_path = original_uri.path();
    let upstream_path = original_path
        .strip_prefix(&provider.route_prefix)
        .unwrap_or(original_path);

    // Build upstream URI
    let upstream_uri = format!(
        "{}{}{}",
        provider.upstream_url.trim_end_matches('/'),
        upstream_path,
        original_uri
            .query()
            .map(|q| format!("?{}", q))
            .unwrap_or_default()
    );

    let uri: Uri = upstream_uri
        .parse()
        .map_err(|e| format!("Invalid upstream URI: {}", e))?;

    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::from(body.clone()))
        .map_err(|e| format!("Failed to build request: {}", e))?;

    // --- Header forwarding (passthrough-first) ---
    // `base_url` in the provider config must include any path prefix required by
    // the upstream API (e.g., `/v1` for OpenAI-compatible endpoints). Without it,
    // the reconstructed upstream URI will be wrong and the provider returns 404.
    // Forward all original headers from the host agent, skipping hop-by-hop
    // and any provider-specific strip_headers.
    let headers = req.headers_mut();
    for (key, value) in original_headers.iter() {
        let key_str = key.as_str().to_lowercase();

        // Skip hop-by-hop headers
        if HOP_BY_HOP_HEADERS.contains(&key_str.as_str()) {
            continue;
        }

        // Skip provider-specific stripped headers
        if provider
            .strip_headers
            .iter()
            .any(|h| h.eq_ignore_ascii_case(&key_str))
        {
            continue;
        }

        headers.insert(key.clone(), value.clone());
    }

    // Always set correct content-length for the (possibly modified) body
    headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&body.len().to_string())
            .map_err(|e| format!("Invalid content-length: {}", e))?,
    );

    // If no content-type was forwarded and we have a body, default to JSON
    if !body.is_empty() && !headers.contains_key(header::CONTENT_TYPE) {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }

    Ok(req)
}

/// Check if the request Content-Type indicates JSON.
/// Returns true for `application/json`, any `+json` suffix, or missing Content-Type
/// (missing Content-Type with a body is treated as potentially JSON for scanning).
fn is_json_content(headers: &HeaderMap) -> bool {
    match headers.get(header::CONTENT_TYPE) {
        None => true, // No Content-Type with body → scan optimistically
        Some(val) => {
            let ct = val.to_str().unwrap_or("");
            let ct_lower = ct.to_lowercase();
            ct_lower.contains("application/json") || ct_lower.contains("+json")
        }
    }
}

/// Check if the response Content-Type indicates Server-Sent Events.
fn is_event_stream(headers: &hyper::HeaderMap) -> bool {
    match headers.get(hyper::header::CONTENT_TYPE) {
        Some(val) => {
            let ct = val.to_str().unwrap_or("");
            ct.to_lowercase().contains("text/event-stream")
        }
        None => false,
    }
}

/// Log per-PII-type match counts without logging actual PII values.
fn log_pii_stats(request_id: &uuid::Uuid, mappings: &crate::mapping::RequestMappings) {
    let mut counts: std::collections::HashMap<PiiType, usize> = std::collections::HashMap::new();
    for mapping in mappings.by_ciphertext.values() {
        *counts.entry(mapping.pii_type).or_insert(0) += 1;
    }

    let total: usize = counts.values().sum();
    let breakdown: Vec<String> = counts
        .iter()
        .map(|(pii_type, count)| format!("{}={}", pii_type, count))
        .collect();

    oo_info!(crate::oo_log::modules::PROXY, "PII encrypted in request",
        request_id = %request_id,
        pii_total = total,
        pii_breakdown = %breakdown.join(", "));
}

/// Orchestrate response integrity scanning with multi-format support.
/// Fail-open: returns original body on any error.
fn scan_response_integrity(
    body: &Bytes,
    pre_extracted_text: Option<&str>,
    format: crate::response_format::ResponseFormat,
    scanner: &ResponseIntegrityScanner,
    request_id: &uuid::Uuid,
    state: &AppState,
) -> Bytes {
    // Use pre-extracted text if available, otherwise extract from body
    let text = match pre_extracted_text {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => match crate::response_format::extract_text(body, format) {
            Some(t) => t,
            None => return body.clone(), // Not parseable or no text content → pass through
        },
    };

    state.health.record_ri_scan();

    // Scan for persuasion techniques
    let report = match scanner.scan(&text) {
        Some(r) => r,
        None => return body.clone(), // Clean or filtered → pass through
    };

    let flag_count = report.flags.len() as u64;
    state.health.record_ri_flags(flag_count);

    let category_names: Vec<String> = {
        let mut names: Vec<String> = report.categories.iter().map(|c| c.to_string()).collect();
        names.sort();
        names
    };

    oo_info!(crate::oo_log::modules::RESPONSE_INTEGRITY, "Persuasion techniques detected",
        request_id = %request_id,
        severity = %report.severity,
        flags = flag_count,
        categories = %category_names.join(", "),
        r2_role = %report.r2_role,
        r2_categories = %report.r2_categories.join(", "),
        scan_time_us = report.scan_time_us);

    // Below min_flags threshold — log but don't inject warning
    let min_flags = state.config.response_integrity.ri_min_flags;
    if (flag_count as usize) < min_flags {
        oo_info!(crate::oo_log::modules::RESPONSE_INTEGRITY,
            "Below min_flags threshold, skipping warning injection",
            request_id = %request_id,
            flags = flag_count,
            min_flags = min_flags);
        return body.clone();
    }

    // R2 Discover reports (R1 clean, R2-only detection) are log-only:
    // they lack R1 corroboration and have high false-positive rate.
    if report.r2_role == crate::response_integrity::R2Role::Discover {
        return body.clone();
    }

    // Inject warning label using multi-format support
    let label = ResponseIntegrityScanner::format_warning_label(&report);
    match crate::response_format::inject_warning(body, format, &label) {
        Some(modified) => modified,
        None => body.clone(), // Fail-open: return original on injection error
    }
}

/// Run RI scan on accumulated SSE text and return a warning as a standard content
/// delta chunk (matching the detected SSE format). Returns an empty string if clean
/// or no scanner.
///
/// Takes `&mut SseRiBuffer` (instead of consuming it) so it can be called inline
/// during stream processing — before `[DONE]` is yielded.
fn emit_sse_ri_warning_inline(
    ri_buffer: &mut crate::sse_accumulator::SseRiBuffer,
    ri_scanner: Option<&ResponseIntegrityScanner>,
    min_flags: usize,
    request_id: &uuid::Uuid,
    health: &HealthStats,
) -> String {
    let scanner = match ri_scanner {
        Some(s) => s,
        None => return String::new(),
    };

    let format = ri_buffer.detected_format();
    let text = match ri_buffer.take_text() {
        Some(t) => t,
        None => return String::new(),
    };

    health.record_ri_scan();

    let report = match scanner.scan(&text) {
        Some(r) => r,
        None => return String::new(), // Clean → no warning
    };

    let flag_count = report.flags.len() as u64;
    health.record_ri_flags(flag_count);

    let category_names: Vec<String> = {
        let mut names: Vec<String> = report.categories.iter().map(|c| c.to_string()).collect();
        names.sort();
        names
    };

    oo_info!(crate::oo_log::modules::RESPONSE_INTEGRITY, "SSE: Persuasion techniques detected",
        request_id = %request_id,
        severity = %report.severity,
        flags = flag_count,
        categories = %category_names.join(", "),
        r2_role = %report.r2_role,
        r2_categories = %report.r2_categories.join(", "),
        scan_time_us = report.scan_time_us);

    // Below min_flags threshold — log but don't inject warning
    if (flag_count as usize) < min_flags {
        oo_info!(crate::oo_log::modules::RESPONSE_INTEGRITY,
            "SSE: Below min_flags threshold, skipping warning injection",
            request_id = %request_id,
            flags = flag_count,
            min_flags = min_flags);
        return String::new();
    }

    // R2 Discover reports (R1 clean, R2-only detection) are log-only:
    // they lack R1 corroboration and have high false-positive rate.
    if report.r2_role == crate::response_integrity::R2Role::Discover {
        return String::new();
    }

    // Emit warning as a standard content delta matching the detected SSE format
    let label = ResponseIntegrityScanner::format_warning_label(&report);
    crate::sse_accumulator::format_sse_warning_chunk(format, &label)
}

/// Inject `X-OO-*` per-feature timing headers into a response.
/// Only emits headers with non-zero values.
fn inject_timing_headers(
    headers: &mut HeaderMap,
    body_timing: (u64, u64, u64, u64, u64, u64, u64, u64),
    ri_us: u64,
    total_us: u64,
) {
    let (scan_us, fpe_us, image_us, voice_us, kws_us, nsfw_ms, face_ms, ocr_ms) = body_timing;

    let pairs: &[(&str, u64)] = &[
        ("x-oo-scan-us", scan_us),
        ("x-oo-fpe-us", fpe_us),
        ("x-oo-image-us", image_us),
        ("x-oo-voice-ms", voice_us / 1000),
        ("x-oo-kws-ms", kws_us / 1000),
        ("x-oo-nsfw-ms", nsfw_ms),
        ("x-oo-face-ms", face_ms),
        ("x-oo-ocr-ms", ocr_ms),
        ("x-oo-ri-us", ri_us),
        ("x-oo-total-us", total_us),
    ];

    for &(name, value) in pairs {
        if value > 0 {
            if let Ok(hv) = HeaderValue::from_str(&value.to_string()) {
                if let Ok(hn) = name.parse::<header::HeaderName>() {
                    headers.insert(hn, hv);
                }
            }
        }
    }
}

/// Check if request body is JSON content (public for testing).
#[cfg(test)]
pub fn is_json_content_pub(headers: &HeaderMap) -> bool {
    is_json_content(headers)
}

/// Buffer the request body with streaming size enforcement.
///
/// Rejects as soon as accumulated bytes exceed the limit, without
/// buffering the full body first.
async fn buffer_body(req: Request<Body>, max_bytes: usize) -> Result<Bytes, StatusCode> {
    let body = req.into_body();
    let mut collected = Vec::new();
    let mut stream = BodyStream::new(body);

    while let Some(frame_result) = stream.next().await {
        let frame = frame_result.map_err(|e| {
            oo_error!(crate::oo_log::modules::PROXY, "Failed to read request body", error = %e);
            StatusCode::BAD_REQUEST
        })?;
        if let Ok(data) = frame.into_data() {
            collected.extend_from_slice(&data);
            if collected.len() > max_bytes {
                oo_warn!(
                    crate::oo_log::modules::PROXY,
                    "Request body exceeds size limit",
                    accumulated = collected.len(),
                    limit = max_bytes
                );
                return Err(StatusCode::PAYLOAD_TOO_LARGE);
            }
        }
    }
    Ok(Bytes::from(collected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AppConfig, ProviderConfig};
    use axum::http::{HeaderMap, HeaderValue, Method, Uri};
    use std::collections::HashMap;

    fn test_config_with_providers() -> AppConfig {
        let mut providers = HashMap::new();
        providers.insert(
            "anthropic".to_string(),
            ProviderConfig {
                upstream_url: "https://api.anthropic.com".to_string(),
                route_prefix: "/anthropic".to_string(),
                strip_headers: vec!["x-internal".to_string()],
            },
        );
        providers.insert(
            "openai".to_string(),
            ProviderConfig {
                upstream_url: "https://api.openai.com".to_string(),
                route_prefix: "/openai".to_string(),
                strip_headers: vec![],
            },
        );
        providers.insert(
            "openai_v2".to_string(),
            ProviderConfig {
                upstream_url: "https://api.openai.com/v2".to_string(),
                route_prefix: "/openai/v2".to_string(),
                strip_headers: vec![],
            },
        );
        AppConfig {
            proxy: crate::config::ProxyConfig::default(),
            providers,
            fpe: crate::config::FpeConfig::default(),
            scanner: crate::config::ScannerConfig::default(),
            logging: crate::config::LoggingConfig::default(),
            image: crate::config::ImageConfig::default(),
            voice: crate::config::VoiceConfig::default(),
            response_integrity: crate::config::ResponseIntegrityConfig::default(),
        }
    }

    // --- response_format extract + inject (via response_format module) ---

    #[test]
    fn test_extract_anthropic_response() {
        let body = serde_json::json!({
            "content": [{"type": "text", "text": "Hello, how can I help you?"}],
            "model": "claude-sonnet-4-6-20250514"
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let format = crate::response_format::detect(Some("application/json"), &bytes);
        let text = crate::response_format::extract_text(&bytes, format);
        assert_eq!(text.unwrap(), "Hello, how can I help you?");
    }

    #[test]
    fn test_extract_openai_response() {
        let body = serde_json::json!({
            "choices": [{"message": {"content": "Hello there!"}, "index": 0}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let format = crate::response_format::detect(Some("application/json"), &bytes);
        let text = crate::response_format::extract_text(&bytes, format);
        assert_eq!(text.unwrap(), "Hello there!");
    }

    #[test]
    fn test_extract_non_json_returns_none() {
        let bytes = b"not json at all";
        let format = crate::response_format::detect(Some("image/png"), bytes.as_slice());
        assert!(crate::response_format::extract_text(bytes.as_slice(), format).is_none());
    }

    #[test]
    fn test_extract_no_text_content() {
        let body = serde_json::json!({"id": "msg_123", "type": "message"});
        let bytes = serde_json::to_vec(&body).unwrap();
        let format = crate::response_format::detect(Some("application/json"), &bytes);
        assert_eq!(format, crate::response_format::ResponseFormat::UnknownJson);
        assert!(crate::response_format::extract_text(&bytes, format).is_none());
    }

    #[test]
    fn test_inject_label_anthropic() {
        let body = serde_json::json!({
            "content": [{"type": "text", "text": "Buy now!"}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let label = "--- OpenObscure WARNING ---\n\
                     Detected: Commercial\n\
                     This content may be designed to manipulate your decision-making.\n\
                     ---\n\n";
        let format = crate::response_format::detect(Some("application/json"), &bytes);
        let result = crate::response_format::inject_warning(&bytes, format, label);
        assert!(result.is_some());
        let modified: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
        let text = modified["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("--- OpenObscure WARNING ---"));
        assert!(text.contains("Buy now!"));
    }

    #[test]
    fn test_inject_label_openai() {
        let body = serde_json::json!({
            "choices": [{"message": {"content": "Act now!"}, "index": 0}]
        });
        let bytes = serde_json::to_vec(&body).unwrap();
        let label = "--- OpenObscure WARNING ---\n\
                     Detected: Fear \u{2022} Urgency\n\
                     Review carefully before acting on it.\n\
                     ---\n\n";
        let format = crate::response_format::detect(Some("application/json"), &bytes);
        let result = crate::response_format::inject_warning(&bytes, format, label);
        assert!(result.is_some());
        let modified: serde_json::Value = serde_json::from_slice(&result.unwrap()).unwrap();
        let text = modified["choices"][0]["message"]["content"]
            .as_str()
            .unwrap();
        assert!(text.starts_with("--- OpenObscure WARNING ---"));
        assert!(text.contains("Act now!"));
    }

    #[test]
    fn test_inject_label_invalid_json() {
        let bytes = b"not json";
        let format = crate::response_format::detect(Some("application/json"), bytes.as_slice());
        assert!(
            crate::response_format::inject_warning(bytes.as_slice(), format, "WARNING: ").is_none()
        );
    }

    // --- resolve_provider ---

    #[test]
    fn test_resolve_provider_matches_prefix() {
        let config = test_config_with_providers();
        let uri: Uri = "/anthropic/v1/messages".parse().unwrap();
        let result = resolve_provider(&config, &uri);
        assert!(result.is_some());
        let (name, provider) = result.unwrap();
        assert_eq!(name, "anthropic");
        assert_eq!(provider.upstream_url, "https://api.anthropic.com");
    }

    #[test]
    fn test_resolve_provider_longest_prefix_wins() {
        let config = test_config_with_providers();
        let uri: Uri = "/openai/v2/chat".parse().unwrap();
        let result = resolve_provider(&config, &uri);
        assert!(result.is_some());
        let (name, _) = result.unwrap();
        assert_eq!(name, "openai_v2");
    }

    #[test]
    fn test_resolve_provider_short_prefix() {
        let config = test_config_with_providers();
        let uri: Uri = "/openai/v1/completions".parse().unwrap();
        let result = resolve_provider(&config, &uri);
        assert!(result.is_some());
        let (name, _) = result.unwrap();
        assert_eq!(name, "openai");
    }

    #[test]
    fn test_resolve_provider_no_match() {
        let config = test_config_with_providers();
        let uri: Uri = "/unknown/api".parse().unwrap();
        let result = resolve_provider(&config, &uri);
        assert!(result.is_none());
    }

    #[test]
    fn test_resolve_provider_root_path_no_match() {
        let config = test_config_with_providers();
        let uri: Uri = "/".parse().unwrap();
        assert!(resolve_provider(&config, &uri).is_none());
    }

    #[test]
    fn test_resolve_provider_empty_providers() {
        let mut config = test_config_with_providers();
        config.providers.clear();
        let uri: Uri = "/anthropic/v1/messages".parse().unwrap();
        assert!(resolve_provider(&config, &uri).is_none());
    }

    // --- is_json_content ---

    #[test]
    fn test_is_json_application_json() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        assert!(is_json_content(&headers));
    }

    #[test]
    fn test_is_json_with_charset() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        assert!(is_json_content(&headers));
    }

    #[test]
    fn test_is_json_plus_json_suffix() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/vnd.api+json"),
        );
        assert!(is_json_content(&headers));
    }

    #[test]
    fn test_is_json_missing_content_type() {
        let headers = HeaderMap::new();
        assert!(is_json_content(&headers));
    }

    #[test]
    fn test_is_not_json_text_plain() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        assert!(!is_json_content(&headers));
    }

    #[test]
    fn test_is_not_json_multipart() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("multipart/form-data"),
        );
        assert!(!is_json_content(&headers));
    }

    #[test]
    fn test_is_not_json_octet_stream() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        assert!(!is_json_content(&headers));
    }

    // --- is_event_stream ---

    #[test]
    fn test_is_event_stream_true() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        assert!(is_event_stream(&headers));
    }

    #[test]
    fn test_is_event_stream_false() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        assert!(!is_event_stream(&headers));
    }

    #[test]
    fn test_is_event_stream_missing() {
        let headers = HeaderMap::new();
        assert!(!is_event_stream(&headers));
    }

    // --- build_upstream_request ---

    #[test]
    fn test_build_upstream_strips_prefix() {
        let provider = ProviderConfig {
            upstream_url: "https://api.anthropic.com".to_string(),
            route_prefix: "/anthropic".to_string(),
            strip_headers: vec![],
        };
        let uri: Uri = "/anthropic/v1/messages".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let body = Bytes::from(r#"{"test":true}"#);
        let vault = Vault::new("test");

        let req = build_upstream_request(
            Method::POST,
            &uri,
            &provider,
            "anthropic",
            &headers,
            &body,
            &vault,
        )
        .unwrap();

        assert_eq!(
            req.uri().to_string(),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(req.method(), Method::POST);
    }

    #[test]
    fn test_build_upstream_preserves_query() {
        let provider = ProviderConfig {
            upstream_url: "https://api.example.com".to_string(),
            route_prefix: "/api".to_string(),
            strip_headers: vec![],
        };
        let uri: Uri = "/api/search?q=test&limit=10".parse().unwrap();
        let headers = HeaderMap::new();
        let body = Bytes::new();
        let vault = Vault::new("test");

        let req =
            build_upstream_request(Method::GET, &uri, &provider, "api", &headers, &body, &vault)
                .unwrap();

        assert!(req.uri().to_string().contains("?q=test&limit=10"));
    }

    #[test]
    fn test_build_upstream_strips_hop_by_hop() {
        let provider = ProviderConfig {
            upstream_url: "https://api.example.com".to_string(),
            route_prefix: "/api".to_string(),
            strip_headers: vec![],
        };
        let uri: Uri = "/api/endpoint".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("keep-alive"));
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        headers.insert("authorization", HeaderValue::from_static("Bearer sk-test"));
        let body = Bytes::from("{}");
        let vault = Vault::new("test");

        let req = build_upstream_request(
            Method::POST,
            &uri,
            &provider,
            "api",
            &headers,
            &body,
            &vault,
        )
        .unwrap();

        // Hop-by-hop headers should be stripped
        assert!(!req.headers().contains_key("connection"));
        assert!(!req.headers().contains_key("keep-alive"));
        assert!(!req.headers().contains_key("transfer-encoding"));
        // Auth header should be forwarded (passthrough-first)
        assert!(req.headers().contains_key("authorization"));
    }

    #[test]
    fn test_build_upstream_strips_provider_headers() {
        let provider = ProviderConfig {
            upstream_url: "https://api.example.com".to_string(),
            route_prefix: "/api".to_string(),
            strip_headers: vec!["x-internal".to_string(), "x-debug".to_string()],
        };
        let uri: Uri = "/api/endpoint".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("x-internal", HeaderValue::from_static("secret"));
        headers.insert("x-debug", HeaderValue::from_static("true"));
        headers.insert("x-custom", HeaderValue::from_static("keep-me"));
        let body = Bytes::from("{}");
        let vault = Vault::new("test");

        let req = build_upstream_request(
            Method::POST,
            &uri,
            &provider,
            "api",
            &headers,
            &body,
            &vault,
        )
        .unwrap();

        assert!(!req.headers().contains_key("x-internal"));
        assert!(!req.headers().contains_key("x-debug"));
        assert!(req.headers().contains_key("x-custom"));
    }

    #[test]
    fn test_build_upstream_sets_content_length() {
        let provider = ProviderConfig {
            upstream_url: "https://api.example.com".to_string(),
            route_prefix: "/api".to_string(),
            strip_headers: vec![],
        };
        let uri: Uri = "/api/endpoint".parse().unwrap();
        let headers = HeaderMap::new();
        let body = Bytes::from(r#"{"messages":[]}"#);
        let vault = Vault::new("test");

        let req = build_upstream_request(
            Method::POST,
            &uri,
            &provider,
            "api",
            &headers,
            &body,
            &vault,
        )
        .unwrap();

        let cl = req
            .headers()
            .get("content-length")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cl, "15");
    }

    #[test]
    fn test_build_upstream_defaults_json_content_type() {
        let provider = ProviderConfig {
            upstream_url: "https://api.example.com".to_string(),
            route_prefix: "/api".to_string(),
            strip_headers: vec![],
        };
        let uri: Uri = "/api/endpoint".parse().unwrap();
        let headers = HeaderMap::new(); // No content-type
        let body = Bytes::from("{}");
        let vault = Vault::new("test");

        let req = build_upstream_request(
            Method::POST,
            &uri,
            &provider,
            "api",
            &headers,
            &body,
            &vault,
        )
        .unwrap();

        assert_eq!(
            req.headers().get("content-type").unwrap().to_str().unwrap(),
            "application/json"
        );
    }

    // --- buffer_body ---

    #[tokio::test]
    async fn test_buffer_body_within_limit() {
        let body = Body::from("hello");
        let req = Request::builder().body(body).unwrap();
        let result = buffer_body(req, 1024).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Bytes::from("hello"));
    }

    #[tokio::test]
    async fn test_buffer_body_exceeds_limit() {
        let body = Body::from("a]".repeat(1000));
        let req = Request::builder().body(body).unwrap();
        let result = buffer_body(req, 100).await;
        assert_eq!(result.unwrap_err(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn test_buffer_body_empty() {
        let body = Body::empty();
        let req = Request::builder().body(body).unwrap();
        let result = buffer_body(req, 1024).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
