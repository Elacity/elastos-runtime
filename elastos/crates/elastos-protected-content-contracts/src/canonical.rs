use sha2::{Digest as _, Sha256};
use thiserror::Error;

use crate::Digest32;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ContractError {
    #[error("wrong canonical domain")]
    WrongDomain,
    #[error("canonical input ended early")]
    UnexpectedEnd,
    #[error("canonical input has trailing bytes")]
    TrailingBytes,
    #[error("invalid UTF-8 in {0}")]
    InvalidUtf8(&'static str),
    #[error("invalid field: {0}")]
    InvalidField(&'static str),
    #[error("field exceeds its v1 limit: {0}")]
    FieldTooLong(&'static str),
}

/// The only supported signed and hashed representation for v1 contracts.
pub trait CanonicalContract: Sized {
    fn canonical_bytes(&self) -> Result<Vec<u8>, ContractError>;
    fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ContractError>;

    fn canonical_hash(&self) -> Result<Digest32, ContractError> {
        Ok(Digest32::new(
            Sha256::digest(self.canonical_bytes()?).into(),
        ))
    }
}

pub(crate) trait CanonicalBody: Sized {
    const DOMAIN: &'static str;

    fn validate(&self) -> Result<(), ContractError>;
    fn encode_fields(&self, encoder: &mut Encoder) -> Result<(), ContractError>;
    fn decode_fields(decoder: &mut Decoder<'_>) -> Result<Self, ContractError>;
}

impl<T: CanonicalBody> CanonicalContract for T {
    fn canonical_bytes(&self) -> Result<Vec<u8>, ContractError> {
        self.validate()?;
        let mut encoder = Encoder::new(T::DOMAIN);
        self.encode_fields(&mut encoder)?;
        Ok(encoder.finish())
    }

    fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, ContractError> {
        let mut decoder = Decoder::new(bytes, T::DOMAIN)?;
        let value = T::decode_fields(&mut decoder)?;
        decoder.finish()?;
        value.validate()?;
        if value.canonical_bytes()?.as_slice() != bytes {
            return Err(ContractError::InvalidField("non-canonical encoding"));
        }
        Ok(value)
    }
}

pub(crate) struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    fn new(domain: &str) -> Self {
        let mut bytes = Vec::with_capacity(domain.len() + 1);
        bytes.extend_from_slice(domain.as_bytes());
        bytes.push(0);
        Self { bytes }
    }

    pub(crate) fn fixed(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub(crate) fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    pub(crate) fn bytes(&mut self, value: &[u8]) -> Result<(), ContractError> {
        let length = u16::try_from(value.len())
            .map_err(|_| ContractError::FieldTooLong("canonical byte string"))?;
        self.u16(length);
        self.fixed(value);
        Ok(())
    }

    pub(crate) fn string(&mut self, value: &str) -> Result<(), ContractError> {
        self.bytes(value.as_bytes())
    }

    pub(crate) fn nested<T: CanonicalContract>(&mut self, value: &T) -> Result<(), ContractError> {
        self.bytes(&value.canonical_bytes()?)
    }

    fn finish(self) -> Vec<u8> {
        self.bytes
    }
}

pub(crate) struct Decoder<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8], domain: &str) -> Result<Self, ContractError> {
        let prefix = domain.as_bytes();
        if bytes.len() <= prefix.len()
            || &bytes[..prefix.len()] != prefix
            || bytes[prefix.len()] != 0
        {
            return Err(ContractError::WrongDomain);
        }
        Ok(Self {
            bytes,
            cursor: prefix.len() + 1,
        })
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], ContractError> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or(ContractError::UnexpectedEnd)?;
        let value = self
            .bytes
            .get(self.cursor..end)
            .ok_or(ContractError::UnexpectedEnd)?;
        self.cursor = end;
        Ok(value)
    }

    pub(crate) fn fixed<const N: usize>(&mut self) -> Result<[u8; N], ContractError> {
        self.take(N)?
            .try_into()
            .map_err(|_| ContractError::UnexpectedEnd)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, ContractError> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, ContractError> {
        Ok(u16::from_be_bytes(self.fixed()?))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, ContractError> {
        Ok(u32::from_be_bytes(self.fixed()?))
    }

    pub(crate) fn u64(&mut self) -> Result<u64, ContractError> {
        Ok(u64::from_be_bytes(self.fixed()?))
    }

    pub(crate) fn bytes(
        &mut self,
        field: &'static str,
        maximum: usize,
    ) -> Result<Vec<u8>, ContractError> {
        let length = usize::from(self.u16()?);
        if length > maximum {
            return Err(ContractError::FieldTooLong(field));
        }
        Ok(self.take(length)?.to_vec())
    }

    pub(crate) fn string(
        &mut self,
        field: &'static str,
        maximum: usize,
    ) -> Result<String, ContractError> {
        String::from_utf8(self.bytes(field, maximum)?)
            .map_err(|_| ContractError::InvalidUtf8(field))
    }

    pub(crate) fn nested<T: CanonicalContract>(
        &mut self,
        field: &'static str,
    ) -> Result<T, ContractError> {
        let bytes = self.bytes(field, u16::MAX as usize)?;
        T::from_canonical_bytes(&bytes)
    }

    fn finish(&self) -> Result<(), ContractError> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(ContractError::TrailingBytes)
        }
    }
}

pub(crate) fn validate_ascii_identifier(
    value: &str,
    field: &'static str,
    maximum: usize,
) -> Result<(), ContractError> {
    if value.is_empty() {
        return Err(ContractError::InvalidField(field));
    }
    if value.len() > maximum {
        return Err(ContractError::FieldTooLong(field));
    }
    if !value.bytes().all(|byte| (0x21..=0x7e).contains(&byte)) {
        return Err(ContractError::InvalidField(field));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RightsPolicyIdentityV1;

    #[test]
    fn canonical_decode_rejects_wrong_domain_truncation_and_trailing_bytes() {
        let policy = RightsPolicyIdentityV1::new(Digest32::new([0x44; 32]), 384).unwrap();
        let canonical = policy.canonical_bytes().unwrap();

        let mut wrong_domain = canonical.clone();
        wrong_domain[0] ^= 1;
        assert_eq!(
            RightsPolicyIdentityV1::from_canonical_bytes(&wrong_domain),
            Err(ContractError::WrongDomain)
        );

        let mut truncated = canonical.clone();
        truncated.pop();
        assert_eq!(
            RightsPolicyIdentityV1::from_canonical_bytes(&truncated),
            Err(ContractError::UnexpectedEnd)
        );

        let mut trailing = canonical;
        trailing.push(0);
        assert_eq!(
            RightsPolicyIdentityV1::from_canonical_bytes(&trailing),
            Err(ContractError::TrailingBytes)
        );
    }

    #[test]
    fn canonical_encoder_rejects_oversized_byte_strings() {
        let mut encoder = Encoder::new("test");
        assert_eq!(
            encoder.bytes(&vec![0; usize::from(u16::MAX) + 1]),
            Err(ContractError::FieldTooLong("canonical byte string"))
        );
    }
}
