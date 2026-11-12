//! Configurable local-account password rules. OAuth, bearer tokens and existing
//! passwords remain unaffected until a password is created or replaced.
use serde::{Deserialize, Serialize};

use crate::{ApiError, AppState};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CountRule {
    pub enabled: bool,
    pub minimum: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PasswordPolicy {
    pub minimum_length_enabled: bool,
    pub minimum_length: usize,
    pub maximum_length_enabled: bool,
    pub maximum_length: usize,
    pub uppercase: CountRule,
    pub lowercase: CountRule,
    pub digits: CountRule,
    pub symbols: CountRule,
    pub reject_whitespace: bool,
    pub reject_username: bool,
    pub reject_email: bool,
    pub reject_common: bool,
    pub reject_repeated: bool,
    pub maximum_repeated: usize,
    pub reject_sequences: bool,
    pub sequence_length: usize,
    pub history_enabled: bool,
    pub history_count: usize,
}

impl Default for PasswordPolicy {
    fn default() -> Self {
        Self {
            minimum_length_enabled: true,
            minimum_length: 12,
            maximum_length_enabled: true,
            maximum_length: 128,
            uppercase: CountRule {
                enabled: false,
                minimum: 1,
            },
            lowercase: CountRule {
                enabled: false,
                minimum: 1,
            },
            digits: CountRule {
                enabled: false,
                minimum: 1,
            },
            symbols: CountRule {
                enabled: false,
                minimum: 1,
            },
            reject_whitespace: false,
            reject_username: true,
            reject_email: true,
            reject_common: true,
            reject_repeated: false,
            maximum_repeated: 3,
            reject_sequences: false,
            sequence_length: 4,
            history_enabled: false,
            history_count: 5,
        }
    }
}

impl PasswordPolicy {
    /// Values are bounded even when optional checks are disabled, so settings
    /// cannot create unbounded hashing work or invalid configurations.
    pub fn validate_configuration(&self) -> Result<(), ApiError> {
        if !(8..=128).contains(&self.minimum_length)
            || !(8..=128).contains(&self.maximum_length)
            || (self.minimum_length_enabled
                && self.maximum_length_enabled
                && self.minimum_length > self.maximum_length)
            || [
                &self.uppercase,
                &self.lowercase,
                &self.digits,
                &self.symbols,
            ]
            .iter()
            .any(|rule| !(1..=32).contains(&rule.minimum))
            || !(2..=12).contains(&self.maximum_repeated)
            || !(3..=12).contains(&self.sequence_length)
            || !(1..=10).contains(&self.history_count)
        {
            return Err(ApiError::bad_request(
                "invalid password policy configuration",
            ));
        }
        let count = [
            &self.uppercase,
            &self.lowercase,
            &self.digits,
            &self.symbols,
        ]
        .iter()
        .filter(|rule| rule.enabled)
        .map(|rule| rule.minimum)
        .sum::<usize>();
        if self.maximum_length_enabled && count > self.maximum_length {
            return Err(ApiError::bad_request(
                "sum of required character counts exceeds maximum password length",
            ));
        }
        Ok(())
    }

    pub fn validate_password(
        &self,
        password: &str,
        username: Option<&str>,
        email: Option<&str>,
    ) -> Result<(), ApiError> {
        // Absolute safety bounds are independent of optional administrator rules.
        // Keep original login/hash behavior compatible with existing accounts.
        let length = password.chars().count();
        if length < 8 || password.len() > 128 || password.chars().any(char::is_control) {
            return Err(ApiError::bad_request(
                "password must be at least 8 characters, at most 128 UTF-8 bytes and have no control characters",
            ));
        }
        if self.minimum_length_enabled && length < self.minimum_length {
            return Err(ApiError::bad_request(
                "password is shorter than the configured minimum",
            ));
        }
        if self.maximum_length_enabled && length > self.maximum_length {
            return Err(ApiError::bad_request(
                "password exceeds the configured maximum",
            ));
        }
        let requirements = [
            (
                &self.uppercase,
                password.chars().filter(|c| c.is_uppercase()).count(),
                "uppercase letters",
            ),
            (
                &self.lowercase,
                password.chars().filter(|c| c.is_lowercase()).count(),
                "lowercase letters",
            ),
            (
                &self.digits,
                password.chars().filter(|c| c.is_numeric()).count(),
                "digits",
            ),
            (
                &self.symbols,
                password
                    .chars()
                    .filter(|c| !c.is_alphanumeric() && !c.is_whitespace())
                    .count(),
                "special characters",
            ),
        ];
        for (rule, count, name) in requirements {
            if rule.enabled && count < rule.minimum {
                return Err(ApiError::bad_request(match name {
                    "uppercase letters" => "password requires more uppercase letters",
                    "lowercase letters" => "password requires more lowercase letters",
                    "digits" => "password requires more digits",
                    _ => "password requires more special characters",
                }));
            }
        }
        if self.reject_whitespace && password.chars().any(char::is_whitespace) {
            return Err(ApiError::bad_request(
                "password must not contain whitespace",
            ));
        }
        let folded = password.to_lowercase();
        if self.reject_username
            && username.is_some_and(|name| name.len() >= 3 && folded.contains(&name.to_lowercase()))
        {
            return Err(ApiError::bad_request(
                "password must not contain the username",
            ));
        }
        if self.reject_email
            && email.is_some_and(|address| {
                let local = address.split('@').next().unwrap_or("");
                (local.chars().count() >= 3 && folded.contains(&local.to_lowercase()))
                    || folded.contains(&address.to_lowercase())
            })
        {
            return Err(ApiError::bad_request(
                "password must not contain the email address",
            ));
        }
        if self.reject_common && is_common_password(&folded) {
            return Err(ApiError::bad_request(
                "password appears in the blocked common-password list",
            ));
        }
        if self.reject_repeated && has_repeated_run(password, self.maximum_repeated) {
            return Err(ApiError::bad_request(
                "password contains too many repeated characters",
            ));
        }
        if self.reject_sequences && has_sequence(password, self.sequence_length) {
            return Err(ApiError::bad_request(
                "password contains a simple ascending or descending sequence",
            ));
        }
        Ok(())
    }
}

fn is_common_password(value: &str) -> bool {
    // Small built-in blocklist, intentionally transparent. This is not a
    // compromised-password database or network breach check.
    const COMMON: &[&str] = &[
        "12345678",
        "123456789",
        "1234567890",
        "123456789012",
        "password",
        "password1",
        "password123",
        "password1234",
        "password12345",
        "qwerty123",
        "qwerty1234",
        "qwertyuiop",
        "admin123",
        "admin1234",
        "admin12345",
        "administrator",
        "letmein123",
        "welcome123",
        "iloveyou123",
        "11111111",
        "00000000",
        "123123123",
        "abc123456",
        "football123",
        "monkey123",
        "dragon123",
        "sunshine123",
        "root1234",
        "changeme123",
        "passw0rd",
        "passw0rd123",
        "password!",
        "password1!",
    ];
    COMMON.contains(&value)
}

fn has_repeated_run(value: &str, allowed: usize) -> bool {
    let mut previous: Option<char> = None;
    let mut run = 0;
    for ch in value.chars() {
        if Some(ch) == previous {
            run += 1;
        } else {
            run = 1;
        }
        if run > allowed {
            return true;
        }
        previous = Some(ch);
    }
    false
}

fn has_sequence(value: &str, minimum: usize) -> bool {
    let mut rising = 1;
    let mut falling = 1;
    let mut previous: Option<char> = None;
    for ch in value.chars() {
        let now = ch.to_ascii_lowercase();
        if let Some(old) = previous {
            if old.is_ascii_alphanumeric()
                && now.is_ascii_alphanumeric()
                && u32::from(now) == u32::from(old) + 1
            {
                rising += 1;
            } else {
                rising = 1;
            }
            if old.is_ascii_alphanumeric()
                && now.is_ascii_alphanumeric()
                && u32::from(old) == u32::from(now) + 1
            {
                falling += 1;
            } else {
                falling = 1;
            }
            if rising >= minimum || falling >= minimum {
                return true;
            }
        }
        previous = Some(now);
    }
    false
}

pub async fn load(state: &AppState) -> Result<PasswordPolicy, ApiError> {
    let value = state.store.load_password_policy().await?;
    let policy = value.map_or_else(
        || Ok(PasswordPolicy::default()),
        |json| {
            serde_json::from_str::<PasswordPolicy>(&json)
                .map_err(|_| ApiError::internal("stored password policy is invalid"))
        },
    )?;
    policy
        .validate_configuration()
        .map_err(|_| ApiError::internal("stored password policy is inconsistent"))?;
    Ok(policy)
}

#[cfg(test)]
mod tests {
    use super::{CountRule, PasswordPolicy, has_repeated_run, has_sequence};
    #[test]
    fn default_is_compatible_with_previous_twelve_character_minimum() {
        let policy = PasswordPolicy::default();
        assert!(
            policy
                .validate_password("correct-horse-battery", None, None)
                .is_ok()
        );
        assert!(policy.validate_password("short", None, None).is_err());
        assert!(
            policy
                .validate_password("123456789012", None, None)
                .is_err()
        );
        assert!(policy.validate_configuration().is_ok());
    }
    #[test]
    fn requirements_are_individually_switchable() {
        let mut policy = PasswordPolicy {
            uppercase: CountRule {
                enabled: true,
                minimum: 2,
            },
            lowercase: CountRule {
                enabled: true,
                minimum: 2,
            },
            digits: CountRule {
                enabled: true,
                minimum: 2,
            },
            symbols: CountRule {
                enabled: true,
                minimum: 2,
            },
            reject_whitespace: true,
            ..Default::default()
        };
        assert!(
            policy
                .validate_password("PaSsword22!!More", None, None)
                .is_ok()
        );
        assert!(
            policy
                .validate_password("Password1!More", None, None)
                .is_err()
        );
        policy.uppercase.enabled = false;
        policy.lowercase.enabled = false;
        policy.digits.enabled = false;
        policy.symbols.enabled = false;
        assert!(
            policy
                .validate_password("longlowercasepass", None, None)
                .is_ok()
        );
    }
    #[test]
    fn blocks_reuse_of_user_data_and_predictable_patterns() {
        let mut policy = PasswordPolicy::default();
        assert!(
            policy
                .validate_password("secretAlice!2026", Some("alice"), None)
                .is_err()
        );
        assert!(
            policy
                .validate_password("someBob@2026Long", None, Some("bob@example.com"))
                .is_err()
        );
        policy.reject_repeated = true;
        policy.maximum_repeated = 2;
        assert!(
            policy
                .validate_password("GoodaaaPass2026!", None, None)
                .is_err()
        );
        policy.reject_sequences = true;
        policy.sequence_length = 4;
        assert!(
            policy
                .validate_password("HiddenAbcdSecret", None, None)
                .is_err()
        );
        assert!(has_repeated_run("aaab", 2));
        assert!(has_sequence("4321", 4));
        assert!(!has_sequence("a1b2", 3));
    }
    #[test]
    fn does_not_allow_absurd_or_conflicting_policies() {
        let mut policy = PasswordPolicy {
            maximum_length: 8,
            ..Default::default()
        };
        assert!(policy.validate_configuration().is_err());
        policy.minimum_length_enabled = false;
        policy.uppercase = CountRule {
            enabled: true,
            minimum: 20,
        };
        assert!(policy.validate_configuration().is_err());
        policy.uppercase.enabled = false;
        assert!(policy.validate_configuration().is_ok());
        policy.history_count = 100;
        assert!(policy.validate_configuration().is_err());
    }
    #[test]
    fn unicode_character_types_and_byte_cap_apply() {
        let mut policy = PasswordPolicy::default();
        policy.uppercase.enabled = true;
        policy.lowercase.enabled = true;
        policy.digits.enabled = true;
        policy.symbols.enabled = true;
        assert!(
            policy
                .validate_password("ПарольОченьХороший4!", None, None)
                .is_ok()
        );
        assert!(
            policy
                .validate_password(&"界".repeat(50), None, None)
                .is_err()
        );
    }
}
