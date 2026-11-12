/** User-facing schema mirrors the server-enforced local password policy.
 * UI never sends passwords to this endpoint, only policy metadata. */
export type CountRule = { enabled: boolean; minimum: number };
export type PasswordPolicy = {
  minimumLengthEnabled: boolean;
  minimumLength: number;
  maximumLengthEnabled: boolean;
  maximumLength: number;
  uppercase: CountRule;
  lowercase: CountRule;
  digits: CountRule;
  symbols: CountRule;
  rejectWhitespace: boolean;
  rejectUsername: boolean;
  rejectEmail: boolean;
  rejectCommon: boolean;
  rejectRepeated: boolean;
  maximumRepeated: number;
  rejectSequences: boolean;
  sequenceLength: number;
  historyEnabled: boolean;
  historyCount: number;
};

export const DEFAULT_PASSWORD_POLICY: PasswordPolicy = {
  minimumLengthEnabled: true, minimumLength: 12,
  maximumLengthEnabled: true, maximumLength: 128,
  uppercase: { enabled: false, minimum: 1 },
  lowercase: { enabled: false, minimum: 1 },
  digits: { enabled: false, minimum: 1 },
  symbols: { enabled: false, minimum: 1 },
  rejectWhitespace: false, rejectUsername: true, rejectEmail: true,
  rejectCommon: true, rejectRepeated: false, maximumRepeated: 3,
  rejectSequences: false, sequenceLength: 4,
  historyEnabled: false, historyCount: 5,
};

export type PasswordPreset = "passphrase" | "balanced" | "strict";

export function presetPolicy(preset: PasswordPreset): PasswordPolicy {
  const policy: PasswordPolicy = structuredClone(DEFAULT_PASSWORD_POLICY);
  if (preset === "balanced" || preset === "strict") {
    for (const key of ["uppercase", "lowercase", "digits", "symbols"] as const) {
      policy[key].enabled = true;
      policy[key].minimum = preset === "strict" ? 2 : 1;
    }
  }
  if (preset === "strict") {
    policy.minimumLength = 16;
    policy.rejectWhitespace = true;
    policy.rejectRepeated = true;
    policy.rejectSequences = true;
    policy.historyEnabled = true;
    policy.historyCount = 5;
  }
  return policy;
}

export async function fetchPasswordPolicy(signal?: AbortSignal): Promise<PasswordPolicy> {
  const response = await fetch("/api/v1/auth/password-policy", { cache: "no-store", signal });
  if (!response.ok) throw new Error("Could not load password policy");
  return await response.json() as PasswordPolicy;
}

/** Lightweight feedback only. The backend remains authoritative. */
export function passwordRuleHints(policy: PasswordPolicy, password: string): Array<{ text: string; satisfied: boolean }> {
  const chars = [...password];
  const rules: Array<{ text: string; satisfied: boolean }> = [];
  if (policy.minimumLengthEnabled) rules.push({
    text: `At least ${policy.minimumLength} characters`,
    satisfied: chars.length >= policy.minimumLength,
  });
  if (policy.maximumLengthEnabled) rules.push({
    text: `No more than ${policy.maximumLength} characters`,
    satisfied: chars.length <= policy.maximumLength,
  });
  for (const [label, rule, pattern] of [
    ["uppercase letters", policy.uppercase, /\p{Lu}/u],
    ["lowercase letters", policy.lowercase, /\p{Ll}/u],
    ["digits", policy.digits, /\p{N}/u],
    ["special characters", policy.symbols, /[^\p{L}\p{N}\s]/u],
  ] as const) {
    if (rule.enabled) rules.push({
      text: `At least ${rule.minimum} ${label}`,
      satisfied: chars.filter((character) => pattern.test(character)).length >= rule.minimum,
    });
  }
  if (policy.rejectWhitespace) rules.push({
    text: "No whitespace", satisfied: !/\s/u.test(password),
  });
  if (policy.rejectRepeated) rules.push({
    text: `No more than ${policy.maximumRepeated} identical characters in a row`,
    satisfied: !new RegExp(`(.)\\1{${policy.maximumRepeated},}`, "u").test(password),
  });
  return rules;
}
