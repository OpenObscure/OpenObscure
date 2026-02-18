/**
 * Memory Governance — Retention tier engine for GDPR data lifecycle management.
 *
 * Manages data retention tiers:
 *   hot    → 7 days  (active conversation data)
 *   warm   → 30 days (recent but inactive)
 *   cold   → 90 days (archive before deletion)
 *   expired → immediate deletion candidate
 *
 * The MemoryGovernor promotes entries through tiers based on age,
 * and prunes expired entries from the database.
 */

import { ConsentManager, RetentionTier } from "./consent-manager";

export interface RetentionPolicy {
  /** Days in hot tier before promotion to warm (default: 7). */
  hotDays: number;
  /** Days in warm tier before promotion to cold (default: 30). */
  warmDays: number;
  /** Days in cold tier before promotion to expired (default: 90). */
  coldDays: number;
}

export const DEFAULT_RETENTION_POLICY: RetentionPolicy = {
  hotDays: 7,
  warmDays: 30,
  coldDays: 90,
};

export interface EnforceResult {
  promoted: number;
  pruned: number;
}

/** Tier promotion order. */
const TIER_TRANSITIONS: Array<{ from: RetentionTier; to: RetentionTier; daysKey: keyof RetentionPolicy }> = [
  { from: "hot", to: "warm", daysKey: "warmDays" },
  { from: "warm", to: "cold", daysKey: "coldDays" },
  { from: "cold", to: "expired", daysKey: "coldDays" },
];

export class MemoryGovernor {
  private manager: ConsentManager;
  private policy: RetentionPolicy;

  constructor(manager: ConsentManager, policy?: Partial<RetentionPolicy>) {
    this.manager = manager;
    this.policy = { ...DEFAULT_RETENTION_POLICY, ...policy };
  }

  /** Run tier promotion + pruning. Returns counts of promoted and pruned entries. */
  enforce(now?: Date): EnforceResult {
    const currentTime = now ?? new Date();
    let promoted = 0;

    for (const transition of TIER_TRANSITIONS) {
      const candidates = this.manager.getRetentionCandidates(
        transition.from,
        currentTime.toISOString()
      );
      for (const entry of candidates) {
        const newExpiresAt = addDays(
          currentTime,
          this.policy[transition.daysKey]
        );
        this.manager.updateRetentionTier(
          entry.id,
          transition.to,
          newExpiresAt.toISOString()
        );
        promoted++;
      }
    }

    const pruned = this.manager.pruneExpired(currentTime.toISOString());

    return { promoted, pruned };
  }

  /** Get retention summary for /privacy retention status. */
  getSummary() {
    return this.manager.getRetentionSummary();
  }

  /** Get the current retention policy. */
  getPolicy(): RetentionPolicy {
    return { ...this.policy };
  }

  /** Track a new processing log entry in the retention system. */
  trackEntry(userId: string, sourceId: number, now?: Date): void {
    const currentTime = now ?? new Date();
    const expiresAt = addDays(currentTime, this.policy.hotDays);
    this.manager.trackRetention(
      userId,
      "data_processing_log",
      sourceId,
      "hot",
      expiresAt.toISOString()
    );
  }
}

/** Add days to a date, returning a new Date. */
function addDays(date: Date, days: number): Date {
  const result = new Date(date.getTime());
  result.setDate(result.getDate() + days);
  return result;
}
