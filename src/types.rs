use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum TypesError {
    #[error("invalid repository name: {0}")]
    InvalidRepoName(String),
    #[error("invalid digest: {0}")]
    InvalidDigest(String),
    #[error("invalid tag name: {0}")]
    InvalidTagName(String),
    #[error("invalid reference: {0}")]
    InvalidReference(String),
    #[error("invalid upload id: {0}")]
    InvalidUploadId(String),
    #[error("invalid tenant id: {0}")]
    InvalidTenantId(String),
}

// ---------------------------------------------------------------------------
// RepoName
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RepoName(String);

impl RepoName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_repo_name(s: &str) -> Result<(), TypesError> {
    if s.is_empty() {
        return Err(TypesError::InvalidRepoName("must not be empty".into()));
    }
    if s.len() > 256 {
        return Err(TypesError::InvalidRepoName(
            "must be at most 256 characters".into(),
        ));
    }
    if s.starts_with('/') || s.ends_with('/') {
        return Err(TypesError::InvalidRepoName(
            "must not start or end with '/'".into(),
        ));
    }
    if s.contains("//") {
        return Err(TypesError::InvalidRepoName(
            "must not contain consecutive '//'".into(),
        ));
    }
    let first = s.as_bytes()[0];
    if !(first.is_ascii_lowercase() || first.is_ascii_digit()) {
        return Err(TypesError::InvalidRepoName(
            "first character must be [a-z0-9]".into(),
        ));
    }
    for ch in s.chars() {
        if !(ch.is_ascii_lowercase()
            || ch.is_ascii_digit()
            || ch == '.'
            || ch == '_'
            || ch == '-'
            || ch == '/')
        {
            return Err(TypesError::InvalidRepoName(format!(
                "invalid character '{ch}'"
            )));
        }
    }
    Ok(())
}

impl TryFrom<&str> for RepoName {
    type Error = TypesError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        validate_repo_name(s)?;
        Ok(Self(s.to_owned()))
    }
}

impl FromStr for RepoName {
    type Err = TypesError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl fmt::Display for RepoName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for RepoName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RepoName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::try_from(s.as_str()).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// DigestAlgorithm
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DigestAlgorithm {
    Sha256,
    Sha512,
}

impl DigestAlgorithm {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
        }
    }

    const fn expected_hex_len(&self) -> usize {
        match self {
            Self::Sha256 => 64,
            Self::Sha512 => 128,
        }
    }
}

impl fmt::Display for DigestAlgorithm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Digest
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Digest {
    algorithm: DigestAlgorithm,
    hex: String,
}

impl Digest {
    #[must_use]
    pub const fn algorithm(&self) -> &DigestAlgorithm {
        &self.algorithm
    }

    #[must_use]
    pub fn hex(&self) -> &str {
        &self.hex
    }
}

fn validate_digest(s: &str) -> Result<Digest, TypesError> {
    let Some((alg_str, hex)) = s.split_once(':') else {
        return Err(TypesError::InvalidDigest(
            "must contain exactly one ':'".into(),
        ));
    };
    // Reject multiple colons
    if hex.contains(':') {
        return Err(TypesError::InvalidDigest(
            "must contain exactly one ':'".into(),
        ));
    }

    let algorithm = match alg_str {
        "sha256" => DigestAlgorithm::Sha256,
        "sha512" => DigestAlgorithm::Sha512,
        _ => {
            return Err(TypesError::InvalidDigest(format!(
                "unsupported algorithm '{alg_str}'"
            )));
        }
    };

    let expected_len = algorithm.expected_hex_len();
    if hex.len() != expected_len {
        return Err(TypesError::InvalidDigest(format!(
            "{alg_str} hex must be {expected_len} characters, got {}",
            hex.len()
        )));
    }

    for ch in hex.chars() {
        if !ch.is_ascii_hexdigit() || ch.is_ascii_uppercase() {
            return Err(TypesError::InvalidDigest(format!(
                "hex must be lowercase hexadecimal, found '{ch}'"
            )));
        }
    }

    Ok(Digest {
        algorithm,
        hex: hex.to_owned(),
    })
}

impl TryFrom<&str> for Digest {
    type Error = TypesError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        validate_digest(s)
    }
}

impl FromStr for Digest {
    type Err = TypesError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algorithm, self.hex)
    }
}

impl Serialize for Digest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::try_from(s.as_str()).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// UploadId
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UploadId(Uuid);

impl UploadId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for UploadId {
    fn default() -> Self {
        Self::new()
    }
}

impl TryFrom<&str> for UploadId {
    type Error = TypesError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let uuid = Uuid::parse_str(s).map_err(|e| TypesError::InvalidUploadId(format!("{e}")))?;
        Ok(Self(uuid))
    }
}

impl FromStr for UploadId {
    type Err = TypesError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl fmt::Display for UploadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serialize for UploadId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for UploadId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::try_from(s.as_str()).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// TenantId
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TenantId(String);

impl TenantId {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn default_tenant() -> Self {
        Self("_default".into())
    }
}

impl TryFrom<&str> for TenantId {
    type Error = TypesError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        if s.is_empty() {
            return Err(TypesError::InvalidTenantId("must not be empty".into()));
        }
        Ok(Self(s.to_owned()))
    }
}

impl FromStr for TenantId {
    type Err = TypesError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for TenantId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TenantId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::try_from(s.as_str()).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// TagName
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TagName(String);

impl TagName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_tag_name(s: &str) -> Result<(), TypesError> {
    if s.is_empty() {
        return Err(TypesError::InvalidTagName("must not be empty".into()));
    }
    if s.len() > 128 {
        return Err(TypesError::InvalidTagName(
            "must be at most 128 characters".into(),
        ));
    }
    let first = s.as_bytes()[0];
    if !(first.is_ascii_alphanumeric()) {
        return Err(TypesError::InvalidTagName(
            "first character must be [a-zA-Z0-9]".into(),
        ));
    }
    for ch in s[1..].chars() {
        if !(ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-') {
            return Err(TypesError::InvalidTagName(format!(
                "invalid character '{ch}'"
            )));
        }
    }
    Ok(())
}

impl TryFrom<&str> for TagName {
    type Error = TypesError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        validate_tag_name(s)?;
        Ok(Self(s.to_owned()))
    }
}

impl FromStr for TagName {
    type Err = TypesError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl fmt::Display for TagName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for TagName {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for TagName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::try_from(s.as_str()).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Reference
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Reference {
    Tag(TagName),
    Digest(Digest),
}

impl TryFrom<&str> for Reference {
    type Error = TypesError;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        // Try digest first: if it contains ':' with a known algorithm prefix
        if let Some((alg, _)) = s.split_once(':') {
            if alg == "sha256" || alg == "sha512" {
                return Digest::try_from(s)
                    .map(Reference::Digest)
                    .map_err(|e| TypesError::InvalidReference(e.to_string()));
            }
        }
        // Fallback to tag
        TagName::try_from(s)
            .map(Reference::Tag)
            .map_err(|e| TypesError::InvalidReference(e.to_string()))
    }
}

impl FromStr for Reference {
    type Err = TypesError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl fmt::Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tag(tag) => write!(f, "{tag}"),
            Self::Digest(digest) => write!(f, "{digest}"),
        }
    }
}

impl Serialize for Reference {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Reference {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::try_from(s.as_str()).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- RepoName --

    #[test]
    fn repo_name_valid() {
        assert!(RepoName::try_from("library/alpine").is_ok());
        assert!(RepoName::try_from("my-org/my-repo").is_ok());
        assert!(RepoName::try_from("a").is_ok());
        assert!(RepoName::try_from("a/b/c").is_ok());
        assert!(RepoName::try_from("repo.name_with-mixed").is_ok());
    }

    #[test]
    fn repo_name_invalid() {
        assert!(RepoName::try_from("").is_err());
        assert!(RepoName::try_from("/leading").is_err());
        assert!(RepoName::try_from("trailing/").is_err());
        assert!(RepoName::try_from("double//slash").is_err());
        assert!(RepoName::try_from("Upper").is_err());
        assert!(RepoName::try_from(".dotfirst").is_err());
        assert!(RepoName::try_from("-dashfirst").is_err());
        assert!(RepoName::try_from("a".repeat(257).as_str()).is_err());
    }

    #[test]
    fn repo_name_display_roundtrip() {
        let name = RepoName::try_from("library/alpine").unwrap();
        assert_eq!(name.to_string(), "library/alpine");
        assert_eq!(name.as_str(), "library/alpine");
    }

    // -- Digest --

    #[test]
    fn digest_valid_sha256() {
        let hex = "a".repeat(64);
        let input = format!("sha256:{hex}");
        let d = Digest::try_from(input.as_str()).unwrap();
        assert_eq!(*d.algorithm(), DigestAlgorithm::Sha256);
        assert_eq!(d.hex(), hex);
        assert_eq!(d.to_string(), input);
    }

    #[test]
    fn digest_valid_sha512() {
        let hex = "b".repeat(128);
        let input = format!("sha512:{hex}");
        let d = Digest::try_from(input.as_str()).unwrap();
        assert_eq!(*d.algorithm(), DigestAlgorithm::Sha512);
        assert_eq!(d.hex(), hex);
    }

    #[test]
    fn digest_invalid() {
        assert!(Digest::try_from("md5:abc").is_err());
        assert!(Digest::try_from("sha256:ABCD").is_err());
        assert!(Digest::try_from("sha256:tooshort").is_err());
        assert!(Digest::try_from("nocolon").is_err());
        assert!(Digest::try_from("sha256:a:b").is_err());

        let upper_hex = "A".repeat(64);
        assert!(Digest::try_from(format!("sha256:{upper_hex}").as_str()).is_err());
    }

    // -- UploadId --

    #[test]
    fn upload_id_new_roundtrip() {
        let id = UploadId::new();
        let s = id.to_string();
        let parsed = UploadId::from_str(&s).unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn upload_id_invalid() {
        assert!(UploadId::try_from("not-a-uuid").is_err());
    }

    // -- TenantId --

    #[test]
    fn tenant_id_valid() {
        assert!(TenantId::try_from("org-123").is_ok());
        let def = TenantId::default_tenant();
        assert_eq!(def.as_str(), "_default");
    }

    #[test]
    fn tenant_id_empty_rejected() {
        assert!(TenantId::try_from("").is_err());
    }

    // -- TagName --

    #[test]
    fn tag_name_valid() {
        assert!(TagName::try_from("latest").is_ok());
        assert!(TagName::try_from("v1.0.0").is_ok());
        assert!(TagName::try_from("v1.0-rc1").is_ok());
        assert!(TagName::try_from("A").is_ok());
    }

    #[test]
    fn tag_name_invalid() {
        assert!(TagName::try_from("").is_err());
        assert!(TagName::try_from(".dotfirst").is_err());
        assert!(TagName::try_from("-dashfirst").is_err());
        assert!(TagName::try_from("has space").is_err());
        assert!(TagName::try_from("a".repeat(129).as_str()).is_err());
    }

    // -- Reference --

    #[test]
    fn reference_parses_digest() {
        let hex = "a".repeat(64);
        let input = format!("sha256:{hex}");
        let r = Reference::from_str(&input).unwrap();
        assert!(matches!(r, Reference::Digest(_)));
        assert_eq!(r.to_string(), input);
    }

    #[test]
    fn reference_parses_tag() {
        let r = Reference::from_str("latest").unwrap();
        assert!(matches!(r, Reference::Tag(_)));
        assert_eq!(r.to_string(), "latest");
    }

    #[test]
    fn reference_invalid() {
        assert!(Reference::from_str("").is_err());
        assert!(Reference::from_str("sha256:tooshort").is_err());
    }

    // -- Serde --

    #[test]
    fn serde_roundtrip_repo_name() {
        let name = RepoName::try_from("library/alpine").unwrap();
        let json = serde_json::to_string(&name).unwrap();
        assert_eq!(json, "\"library/alpine\"");
        let parsed: RepoName = serde_json::from_str(&json).unwrap();
        assert_eq!(name, parsed);
    }

    #[test]
    fn serde_roundtrip_digest() {
        let hex = "c".repeat(64);
        let input = format!("sha256:{hex}");
        let d = Digest::from_str(&input).unwrap();
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, format!("\"{input}\""));
        let parsed: Digest = serde_json::from_str(&json).unwrap();
        assert_eq!(d, parsed);
    }

    #[test]
    fn serde_roundtrip_reference() {
        let r = Reference::from_str("latest").unwrap();
        let json = serde_json::to_string(&r).unwrap();
        let parsed: Reference = serde_json::from_str(&json).unwrap();
        assert_eq!(r, parsed);
    }

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            #[test]
            fn repo_name_never_panics(s in "\\PC{0,300}") {
                let _ = RepoName::try_from(s.as_str());
            }

            #[test]
            fn repo_name_roundtrip(s in "[a-z][a-z0-9._/-]{0,100}") {
                prop_assume!(!s.contains("//"));
                prop_assume!(!s.ends_with('/'));
                prop_assume!(s.len() <= 256);
                let name = RepoName::try_from(s.as_str()).unwrap();
                assert_eq!(name.as_str(), s);
                assert_eq!(name.to_string(), s);
            }

            #[test]
            fn repo_name_rejects_uppercase(s in "[A-Z][a-zA-Z0-9]{0,10}") {
                assert!(RepoName::try_from(s.as_str()).is_err());
            }

            #[test]
            fn repo_name_rejects_too_long(s in "[a-z][a-z0-9]{256,400}") {
                assert!(RepoName::try_from(s.as_str()).is_err());
            }

            #[test]
            fn digest_never_panics(s in "\\PC{0,200}") {
                let _ = Digest::try_from(s.as_str());
            }

            #[test]
            fn digest_sha256_roundtrip(hex in "[a-f0-9]{64}") {
                let input = format!("sha256:{hex}");
                let d = Digest::try_from(input.as_str()).unwrap();
                assert_eq!(d.to_string(), input);
                assert_eq!(*d.algorithm(), DigestAlgorithm::Sha256);
            }

            #[test]
            fn digest_sha512_roundtrip(hex in "[a-f0-9]{128}") {
                let input = format!("sha512:{hex}");
                let d = Digest::try_from(input.as_str()).unwrap();
                assert_eq!(d.to_string(), input);
                assert_eq!(*d.algorithm(), DigestAlgorithm::Sha512);
            }

            #[test]
            fn digest_rejects_uppercase_hex(hex in "[A-F0-9]{64}") {
                let input = format!("sha256:{hex}");
                assert!(Digest::try_from(input.as_str()).is_err());
            }

            #[test]
            fn digest_rejects_wrong_length(hex in "[a-f0-9]{1,63}") {
                let input = format!("sha256:{hex}");
                assert!(Digest::try_from(input.as_str()).is_err());
            }

            #[test]
            fn digest_rejects_unknown_algorithm(alg in "[a-z]{3,10}", hex in "[a-f0-9]{64}") {
                prop_assume!(alg != "sha256");
                let input = format!("{alg}:{hex}");
                // sha512 with 64 chars should also fail (wrong length)
                assert!(Digest::try_from(input.as_str()).is_err());
            }

            #[test]
            fn tag_name_never_panics(s in "\\PC{0,200}") {
                let _ = TagName::try_from(s.as_str());
            }

            #[test]
            fn tag_name_roundtrip(s in "[a-zA-Z0-9][a-zA-Z0-9._-]{0,100}") {
                prop_assume!(s.len() <= 128);
                let tag = TagName::try_from(s.as_str()).unwrap();
                assert_eq!(tag.as_str(), s);
            }

            #[test]
            fn tag_name_rejects_too_long(s in "[a-z][a-z0-9]{128,200}") {
                assert!(TagName::try_from(s.as_str()).is_err());
            }

            #[test]
            fn tag_name_rejects_dot_start(s in "\\.[a-z]{1,10}") {
                assert!(TagName::try_from(s.as_str()).is_err());
            }

            #[test]
            fn tag_name_rejects_dash_start(s in "-[a-z]{1,10}") {
                assert!(TagName::try_from(s.as_str()).is_err());
            }

            #[test]
            fn reference_parses_digest_or_tag(s in "[a-z][a-z0-9._-]{0,50}") {
                let r = Reference::try_from(s.as_str());
                assert!(r.is_ok());
                assert!(matches!(r.unwrap(), Reference::Tag(_)));
            }

            #[test]
            fn reference_parses_sha256_digest(hex in "[a-f0-9]{64}") {
                let input = format!("sha256:{hex}");
                let r = Reference::try_from(input.as_str()).unwrap();
                assert!(matches!(r, Reference::Digest(_)));
            }
        }
    }
}
