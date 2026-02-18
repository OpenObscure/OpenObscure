/**
 * Privacy Slash Command Handlers
 *
 * Implements /privacy commands for GDPR consent management:
 *   /privacy status       — Show current consent state and data summary
 *   /privacy consent grant — Grant consent for data processing
 *   /privacy consent revoke — Revoke consent
 *   /privacy export        — Export all personal data (DSAR access)
 *   /privacy delete        — Request data erasure (DSAR erasure)
 *   /privacy disclosure    — Show AI model disclosure (Art. 13/14)
 */

import {
  ConsentManager,
  ConsentType,
  aiDisclosureText,
} from "./consent-manager";
import { MemoryGovernor } from "./memory-governance";
import * as fs from "fs";
import * as path from "path";

export interface PrivacyCommandResult {
  text: string;
  success: boolean;
}

const VALID_CONSENT_TYPES: ConsentType[] = [
  "processing",
  "storage",
  "transfer",
  "ai_disclosure",
];

export interface PrivacyCommandOptions {
  exportDir?: string;
  governor?: MemoryGovernor;
}

/**
 * Route a /privacy command to the appropriate handler.
 * Returns formatted text output for the user.
 */
export function handlePrivacyCommand(
  manager: ConsentManager,
  userId: string,
  args: string[],
  exportDirOrOptions?: string | PrivacyCommandOptions
): PrivacyCommandResult {
  // Backward-compatible: accept string (exportDir) or options object
  const opts: PrivacyCommandOptions =
    typeof exportDirOrOptions === "string"
      ? { exportDir: exportDirOrOptions }
      : exportDirOrOptions ?? {};

  const subcommand = args[0]?.toLowerCase();

  switch (subcommand) {
    case "status":
      return handleStatus(manager, userId);

    case "consent": {
      const action = args[1]?.toLowerCase();
      const consentType = args[2]?.toLowerCase() as ConsentType | undefined;
      if (action === "grant") {
        return handleConsentGrant(manager, userId, consentType);
      } else if (action === "revoke") {
        return handleConsentRevoke(manager, userId, consentType);
      }
      return {
        text: "Usage: /privacy consent <grant|revoke> [type]\nTypes: processing, storage, transfer, ai_disclosure",
        success: false,
      };
    }

    case "export":
      return handleExport(manager, userId, opts.exportDir);

    case "delete":
      return handleDelete(manager, userId);

    case "disclosure":
      return handleDisclosure(args[1], args[2]);

    case "retention": {
      const retentionAction = args[1]?.toLowerCase();
      return handleRetention(retentionAction, opts.governor);
    }

    default:
      return {
        text: [
          "OpenObscure Privacy Commands:",
          "  /privacy status             — Show consent state and data summary",
          "  /privacy consent grant      — Grant consent for data processing",
          "  /privacy consent revoke     — Revoke consent",
          "  /privacy export             — Export all your personal data",
          "  /privacy delete             — Request erasure of all your data",
          "  /privacy disclosure         — Show AI model privacy disclosure",
          "  /privacy retention status   — Show retention tier counts",
          "  /privacy retention enforce  — Run tier promotion + pruning now",
          "  /privacy retention policy   — Show current retention policy",
        ].join("\n"),
        success: true,
      };
  }
}

function handleStatus(
  manager: ConsentManager,
  userId: string
): PrivacyCommandResult {
  const status = manager.getStatus(userId);

  const lines: string[] = ["OpenObscure Privacy Status", ""];

  // Active consents
  const active = status.consents.filter(
    (c) => c.granted && !c.revoked_at
  );
  const revoked = status.consents.filter(
    (c) => !c.granted || c.revoked_at
  );

  if (active.length > 0) {
    lines.push("Active Consents:");
    for (const c of active) {
      const basis = c.legal_basis ? ` (basis: ${c.legal_basis})` : "";
      const purpose = c.purpose ? ` — ${c.purpose}` : "";
      lines.push(
        `  [granted] ${c.consent_type}${basis}${purpose} (v${c.version})`
      );
    }
  } else {
    lines.push("Active Consents: none");
  }

  if (revoked.length > 0) {
    lines.push("Revoked Consents:");
    for (const c of revoked) {
      lines.push(`  [revoked] ${c.consent_type} (revoked: ${c.revoked_at})`);
    }
  }

  lines.push("");
  lines.push(`Data Processing Log: ${status.processing_log_count} entries`);
  lines.push(`Pending DSARs: ${status.pending_dsars}`);

  return { text: lines.join("\n"), success: true };
}

function handleConsentGrant(
  manager: ConsentManager,
  userId: string,
  consentType?: ConsentType
): PrivacyCommandResult {
  // Default to "processing" if no type specified
  const type = consentType ?? "processing";

  if (!VALID_CONSENT_TYPES.includes(type)) {
    return {
      text: `Invalid consent type: "${type}". Valid types: ${VALID_CONSENT_TYPES.join(", ")}`,
      success: false,
    };
  }

  const record = manager.grantConsent(userId, type, "User-initiated consent");
  manager.logProcessing(userId, "store", [], "consent_manager", {
    action: "consent_grant",
    consent_type: type,
  });

  return {
    text: `Consent granted for "${type}" (version ${record.version}). You can revoke at any time with /privacy consent revoke ${type}.`,
    success: true,
  };
}

function handleConsentRevoke(
  manager: ConsentManager,
  userId: string,
  consentType?: ConsentType
): PrivacyCommandResult {
  const type = consentType ?? "processing";

  if (!VALID_CONSENT_TYPES.includes(type)) {
    return {
      text: `Invalid consent type: "${type}". Valid types: ${VALID_CONSENT_TYPES.join(", ")}`,
      success: false,
    };
  }

  const revoked = manager.revokeConsent(userId, type);
  if (!revoked) {
    return {
      text: `No active consent for "${type}" to revoke.`,
      success: false,
    };
  }

  manager.logProcessing(userId, "store", [], "consent_manager", {
    action: "consent_revoke",
    consent_type: type,
  });

  return {
    text: `Consent for "${type}" has been revoked. Non-essential data processing for this category will stop.`,
    success: true,
  };
}

function handleExport(
  manager: ConsentManager,
  userId: string,
  exportDir?: string
): PrivacyCommandResult {
  const data = manager.exportUserData(userId);

  // Create DSAR access request
  const dsar = manager.createDsar(userId, "access");

  if (exportDir) {
    const dir = path.resolve(exportDir);
    if (!fs.existsSync(dir)) {
      fs.mkdirSync(dir, { recursive: true });
    }
    const filePath = path.join(
      dir,
      `privacy-export-${userId}-${Date.now()}.json`
    );
    fs.writeFileSync(filePath, JSON.stringify(data, null, 2));
    manager.updateDsarStatus(dsar.id, "completed", filePath);

    return {
      text: `Data export complete. File saved to: ${filePath}\nDSAR request #${dsar.id} fulfilled.`,
      success: true,
    };
  }

  // No export dir — return inline
  manager.updateDsarStatus(dsar.id, "completed");
  const summary = [
    `Data Export for ${userId}:`,
    `  Consent records: ${data.consents.length}`,
    `  Processing log entries: ${data.processing_log.length}`,
    `  DSAR requests: ${data.dsar_requests.length}`,
    `  DSAR request #${dsar.id} fulfilled.`,
  ];

  return { text: summary.join("\n"), success: true };
}

function handleDelete(
  manager: ConsentManager,
  userId: string
): PrivacyCommandResult {
  // Create DSAR erasure request
  const dsar = manager.createDsar(userId, "erasure");

  const deletedCount = manager.deleteUserData(userId);
  // Note: the DSAR record itself was just deleted too — re-create a completion record
  const completionDsar = manager.createDsar(userId, "erasure");
  manager.updateDsarStatus(completionDsar.id, "completed");

  return {
    text: `Data erasure complete. ${deletedCount} records deleted across all tables.\nDSAR erasure request fulfilled.`,
    success: true,
  };
}

function handleDisclosure(
  modelName?: string,
  provider?: string
): PrivacyCommandResult {
  const model = modelName ?? "the AI model";
  const prov = provider ?? "the configured provider";
  return {
    text: aiDisclosureText(model, prov),
    success: true,
  };
}

function handleRetention(
  action?: string,
  governor?: MemoryGovernor
): PrivacyCommandResult {
  if (!governor) {
    return {
      text: "Memory governance is not enabled. Set memoryGovernance: true in plugin config.",
      success: false,
    };
  }

  switch (action) {
    case "status": {
      const summary = governor.getSummary();
      const lines = [
        "Retention Tier Summary:",
        `  hot:     ${summary.hot} entries`,
        `  warm:    ${summary.warm} entries`,
        `  cold:    ${summary.cold} entries`,
        `  expired: ${summary.expired} entries`,
        `  total:   ${summary.total} entries`,
      ];
      return { text: lines.join("\n"), success: true };
    }

    case "enforce": {
      const result = governor.enforce();
      return {
        text: `Retention enforcement complete. Promoted: ${result.promoted}, Pruned: ${result.pruned}.`,
        success: true,
      };
    }

    case "policy": {
      const policy = governor.getPolicy();
      const lines = [
        "Retention Policy:",
        `  hot  → warm:    after ${policy.hotDays} days`,
        `  warm → cold:    after ${policy.warmDays} days`,
        `  cold → expired: after ${policy.coldDays} days`,
        `  expired: deleted on next enforcement run`,
      ];
      return { text: lines.join("\n"), success: true };
    }

    default:
      return {
        text: "Usage: /privacy retention <status|enforce|policy>",
        success: false,
      };
  }
}
