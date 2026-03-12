// OpenObscureInterceptor.kt — OkHttp interceptor template for Android
// Intercepts outbound chat requests and sanitizes user messages in-flight.
// Wire into your OkHttpClient builder: .addInterceptor(OpenObscureInterceptor())

package your.app.package

import kotlinx.serialization.json.*
import okhttp3.Interceptor
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import okio.Buffer
import uniffi.openobscure_core.*

class OpenObscureInterceptor : Interceptor {

    override fun intercept(chain: Interceptor.Chain): Response {
        var request = chain.request()

        val body = request.body ?: return chain.proceed(request)
        val contentType = body.contentType()
        if (contentType?.subtype != "json") return chain.proceed(request)

        val buffer = Buffer()
        body.writeTo(buffer)
        val bodyStr = buffer.readUtf8()

        val sanitizedBody = sanitizeRequestJson(bodyStr)
        val newRequest = request.newBuilder()
            .method(request.method, sanitizedBody.toRequestBody(contentType))
            .build()

        return chain.proceed(newRequest)
    }

    private fun sanitizeRequestJson(json: String): String {
        val root = try {
            Json.parseToJsonElement(json).jsonObject.toMutableMap()
        } catch (_: Exception) {
            return json
        }

        val messages = root["messages"]?.jsonArray ?: return json
        val mgr = OpenObscureManager

        val sanitizedMessages = messages.map { msg ->
            val obj = msg.jsonObject
            val content = obj["content"]?.jsonPrimitive?.contentOrNull ?: return@map msg

            val result = mgr.sanitize(content)
            if (result.piiCount > 0u) {
                JsonObject(obj.toMutableMap().apply {
                    put("content", JsonPrimitive(result.sanitizedText))
                })
            } else {
                msg
            }
        }

        root["messages"] = JsonArray(sanitizedMessages)
        return JsonObject(root).toString()
    }
}
