use crate::{Error, ExchangeId, Limits, Result};
use std::io::{Cursor, Read};

const PAYLOAD_MAGIC: &[u8; 4] = b"LEM1";
const PAYLOAD_SCHEMA_V1: u16 = 1;
const ATTRIBUTE_REQUIRED: u8 = 1;
const MAX_ATTRIBUTES: usize = 128;
const MAX_ATTRIBUTE_KEY_LENGTH: usize = 128;

const TS_RESPONSE_MS: &[u8] = b"alex.ts_response_ms";
const TS_REQUEST_MS: &[u8] = b"alex.ts_request_ms";
const HARNESS: &[u8] = b"alex.harness";
const CLIENT_FORMAT: &[u8] = b"alex.client_format";
const UPSTREAM_FORMAT: &[u8] = b"alex.upstream_format";
const METHOD: &[u8] = b"http.method";
const PATH: &[u8] = b"http.path";
const STREAMED: &[u8] = b"http.streamed";
const STATUS: &[u8] = b"http.status";
const COST_USD_F64_BITS: &[u8] = b"alex.cost_usd_f64_bits";
const BILLING_BUCKET: &[u8] = b"alex.billing_bucket";
const ERROR_KIND: &[u8] = b"alex.error.kind";
const ERROR_CODE: &[u8] = b"alex.error.code";
const SUBSTITUTED: &[u8] = b"alex.substituted";
const ORIGINAL_MODEL: &[u8] = b"alex.original_model";
const SERVED_MODEL: &[u8] = b"alex.served_model";
const SUBSTITUTION_REASON: &[u8] = b"alex.substitution_reason";
const INJECTED: &[u8] = b"alex.injected";
const FIXTURE_NAME: &[u8] = b"alex.fixture_name";
const ATTEMPTS_JSON: &[u8] = b"alex.attempts_json";
const ORIGINAL_ACCOUNT_ID: &[u8] = b"alex.original_account_id";
const SERVED_ACCOUNT_ID: &[u8] = b"alex.served_account_id";
const SUBSCRIPTION_IDENTITY: &[u8] = b"alex.subscription_identity";
const VIA_DARIO: &[u8] = b"alex.via_dario";
const DARIO_GENERATION: &[u8] = b"alex.dario_generation";
const TAGS_JSON: &[u8] = b"alex.tags_json";
const CLIENT_IP: &[u8] = b"network.client_ip";
const KEY_FINGERPRINT: &[u8] = b"alex.key_fingerprint";
const REASONING_EFFORT: &[u8] = b"gen_ai.reasoning_effort";
const THINKING_BUDGET: &[u8] = b"gen_ai.thinking_budget";
const CACHE_CREATION_TOKENS: &[u8] = b"gen_ai.cache_creation_tokens";
const INPUT_TOKENS: &[u8] = b"gen_ai.input_tokens";
const CACHED_INPUT_TOKENS: &[u8] = b"gen_ai.cached_input_tokens";
const OUTPUT_TOKENS: &[u8] = b"gen_ai.output_tokens";
const REASONING_TOKENS: &[u8] = b"gen_ai.reasoning_tokens";

/// One extension attribute not understood by this version of the crate.
///
/// Optional attributes are retained when a record is decoded and encoded
/// again. Required unknown attributes are rejected during decoding, allowing
/// future writers to declare metadata that must not be silently ignored.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnknownExchangeMetadataAttribute {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

/// Transport and exchange metadata that is intentionally outside the stable,
/// content-addressed [`crate::Exchange`] encoding.
///
/// Byte strings avoid imposing UTF-8 normalization on captured values. Raw
/// request/response bodies and header blocks do not belong here: those remain
/// single-copy manifest and header-table references on stages.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExchangeMetadataData {
    pub ts_request_ms: Option<i64>,
    pub ts_response_ms: Option<i64>,
    pub harness: Option<Vec<u8>>,
    pub client_format: Option<Vec<u8>>,
    pub upstream_format: Option<Vec<u8>>,
    pub method: Option<Vec<u8>>,
    pub path: Option<Vec<u8>>,
    pub streamed: Option<bool>,
    pub status: Option<i64>,
    /// Exact IEEE-754 bits from TraceRecord.cost_usd. Stage cost_nanos is a
    /// convenient normalized value but cannot preserve every legacy value.
    pub cost_usd_bits: Option<u64>,
    pub billing_bucket: Option<Vec<u8>>,
    pub error_kind: Option<Vec<u8>>,
    pub error_code: Option<Vec<u8>>,
    pub substituted: bool,
    pub original_model: Option<Vec<u8>>,
    pub served_model: Option<Vec<u8>>,
    pub substitution_reason: Option<Vec<u8>>,
    pub injected: bool,
    pub fixture_name: Option<Vec<u8>>,
    pub attempts_json: Option<Vec<u8>>,
    pub original_account_id: Option<Vec<u8>>,
    pub served_account_id: Option<Vec<u8>>,
    pub subscription_identity: Option<Vec<u8>>,
    pub via_dario: bool,
    pub dario_generation: Option<Vec<u8>>,
    pub tags_json: Option<Vec<u8>>,
    pub client_ip: Option<Vec<u8>>,
    pub key_fingerprint: Option<Vec<u8>>,
    pub reasoning_effort: Option<Vec<u8>>,
    pub thinking_budget: Option<i64>,
    pub input_tokens: Option<i64>,
    pub cached_input_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub unknown_attributes: Vec<UnknownExchangeMetadataAttribute>,
}

/// Optional companion record for exactly one exchange.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExchangeMetadata {
    pub exchange_id: ExchangeId,
    pub data: ExchangeMetadataData,
}

#[derive(Clone)]
struct EncodedAttribute {
    key: Vec<u8>,
    value: Vec<u8>,
    required: bool,
}

impl ExchangeMetadata {
    pub fn new(exchange_id: ExchangeId, data: ExchangeMetadataData) -> Self {
        Self { exchange_id, data }
    }

    pub(crate) fn validate_shape(&self, limits: &Limits) -> Result<()> {
        let _ = self.attributes(limits)?;
        Ok(())
    }

    pub(crate) fn encode(&self, limits: &Limits) -> Result<Vec<u8>> {
        let attributes = self.attributes(limits)?;
        let count = u16::try_from(attributes.len())
            .map_err(|_| Error::Invalid("exchange metadata attribute count exceeds u16"))?;
        let mut out = Vec::new();
        out.extend_from_slice(&self.exchange_id.0);
        out.extend_from_slice(PAYLOAD_MAGIC);
        out.extend_from_slice(&PAYLOAD_SCHEMA_V1.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        for attribute in attributes {
            out.push(if attribute.required {
                ATTRIBUTE_REQUIRED
            } else {
                0
            });
            out.push(0);
            out.extend_from_slice(&(attribute.key.len() as u16).to_le_bytes());
            out.extend_from_slice(&(attribute.value.len() as u32).to_le_bytes());
            out.extend_from_slice(&attribute.key);
            out.extend_from_slice(&attribute.value);
        }
        if out.len() as u64 > limits.max_frame_payload {
            return Err(Error::Limit {
                what: "exchange metadata record",
                actual: out.len() as u64,
                limit: limits.max_frame_payload,
            });
        }
        Ok(out)
    }

    pub(crate) fn decode(bytes: &[u8], limits: &Limits) -> Result<Self> {
        if bytes.len() as u64 > limits.max_frame_payload {
            return Err(Error::Limit {
                what: "exchange metadata record",
                actual: bytes.len() as u64,
                limit: limits.max_frame_payload,
            });
        }
        let mut input = Cursor::new(bytes);
        let exchange_id = ExchangeId(read_array(&mut input)?);
        if read_array::<4>(&mut input)? != *PAYLOAD_MAGIC {
            return Err(Error::Invalid("invalid exchange metadata magic"));
        }
        let schema = read_u16(&mut input)?;
        if schema != PAYLOAD_SCHEMA_V1 {
            return Err(Error::Unsupported(format!(
                "exchange metadata payload schema {schema}"
            )));
        }
        let count = read_u16(&mut input)? as usize;
        if count > MAX_ATTRIBUTES {
            return Err(Error::Limit {
                what: "exchange metadata attribute count",
                actual: count as u64,
                limit: MAX_ATTRIBUTES as u64,
            });
        }
        let mut data = ExchangeMetadataData::default();
        let mut previous_key: Option<Vec<u8>> = None;
        for _ in 0..count {
            let flags = read_u8(&mut input)?;
            if flags & !ATTRIBUTE_REQUIRED != 0 {
                return Err(Error::Unsupported(format!(
                    "exchange metadata attribute flags {flags:#x}"
                )));
            }
            if read_u8(&mut input)? != 0 {
                return Err(Error::Invalid(
                    "exchange metadata attribute reserved byte is nonzero",
                ));
            }
            let key_length = read_u16(&mut input)? as usize;
            let value_length = read_u32(&mut input)? as usize;
            validate_key_length(key_length, limits)?;
            validate_value_length(value_length, limits)?;
            let key = read_bytes(&mut input, key_length)?;
            let value = read_bytes(&mut input, value_length)?;
            if key.is_empty() {
                return Err(Error::Invalid("exchange metadata key is empty"));
            }
            if previous_key
                .as_deref()
                .is_some_and(|previous| previous >= key.as_slice())
            {
                return Err(Error::Invalid(
                    "exchange metadata keys are not strictly sorted",
                ));
            }
            previous_key = Some(key.clone());
            if !decode_known(&mut data, &key, value.clone())? {
                validate_unknown_key(&key)?;
                if flags & ATTRIBUTE_REQUIRED != 0 {
                    return Err(Error::Unsupported(format!(
                        "required exchange metadata attribute {}",
                        String::from_utf8_lossy(&key)
                    )));
                }
                data.unknown_attributes
                    .push(UnknownExchangeMetadataAttribute { key, value });
            }
        }
        if input.position() != bytes.len() as u64 {
            return Err(Error::Invalid("trailing exchange metadata bytes"));
        }
        let value = Self { exchange_id, data };
        value.validate_shape(limits)?;
        Ok(value)
    }

    fn attributes(&self, limits: &Limits) -> Result<Vec<EncodedAttribute>> {
        let mut attributes = Vec::new();
        push_i64(&mut attributes, TS_REQUEST_MS, self.data.ts_request_ms);
        push_i64(&mut attributes, TS_RESPONSE_MS, self.data.ts_response_ms);
        push_bytes(&mut attributes, HARNESS, self.data.harness.as_deref());
        push_bytes(
            &mut attributes,
            CLIENT_FORMAT,
            self.data.client_format.as_deref(),
        );
        push_bytes(
            &mut attributes,
            UPSTREAM_FORMAT,
            self.data.upstream_format.as_deref(),
        );
        push_bytes(&mut attributes, METHOD, self.data.method.as_deref());
        push_bytes(&mut attributes, PATH, self.data.path.as_deref());
        push_bool(&mut attributes, STREAMED, self.data.streamed);
        push_i64(&mut attributes, STATUS, self.data.status);
        push_u64(&mut attributes, COST_USD_F64_BITS, self.data.cost_usd_bits);
        push_bytes(
            &mut attributes,
            BILLING_BUCKET,
            self.data.billing_bucket.as_deref(),
        );
        push_bytes(&mut attributes, ERROR_KIND, self.data.error_kind.as_deref());
        push_bytes(&mut attributes, ERROR_CODE, self.data.error_code.as_deref());
        push_bool(
            &mut attributes,
            SUBSTITUTED,
            self.data.substituted.then_some(true),
        );
        push_bytes(
            &mut attributes,
            ORIGINAL_MODEL,
            self.data.original_model.as_deref(),
        );
        push_bytes(
            &mut attributes,
            SERVED_MODEL,
            self.data.served_model.as_deref(),
        );
        push_bytes(
            &mut attributes,
            SUBSTITUTION_REASON,
            self.data.substitution_reason.as_deref(),
        );
        push_bool(
            &mut attributes,
            INJECTED,
            self.data.injected.then_some(true),
        );
        push_bytes(
            &mut attributes,
            FIXTURE_NAME,
            self.data.fixture_name.as_deref(),
        );
        push_bytes(
            &mut attributes,
            ATTEMPTS_JSON,
            self.data.attempts_json.as_deref(),
        );
        push_bytes(
            &mut attributes,
            ORIGINAL_ACCOUNT_ID,
            self.data.original_account_id.as_deref(),
        );
        push_bytes(
            &mut attributes,
            SERVED_ACCOUNT_ID,
            self.data.served_account_id.as_deref(),
        );
        push_bytes(
            &mut attributes,
            SUBSCRIPTION_IDENTITY,
            self.data.subscription_identity.as_deref(),
        );
        push_bool(
            &mut attributes,
            VIA_DARIO,
            self.data.via_dario.then_some(true),
        );
        push_bytes(
            &mut attributes,
            DARIO_GENERATION,
            self.data.dario_generation.as_deref(),
        );
        push_bytes(&mut attributes, TAGS_JSON, self.data.tags_json.as_deref());
        push_bytes(&mut attributes, CLIENT_IP, self.data.client_ip.as_deref());
        push_bytes(
            &mut attributes,
            KEY_FINGERPRINT,
            self.data.key_fingerprint.as_deref(),
        );
        push_bytes(
            &mut attributes,
            REASONING_EFFORT,
            self.data.reasoning_effort.as_deref(),
        );
        push_i64(&mut attributes, THINKING_BUDGET, self.data.thinking_budget);
        push_i64(&mut attributes, INPUT_TOKENS, self.data.input_tokens);
        push_i64(
            &mut attributes,
            CACHED_INPUT_TOKENS,
            self.data.cached_input_tokens,
        );
        push_i64(
            &mut attributes,
            CACHE_CREATION_TOKENS,
            self.data.cache_creation_tokens,
        );
        push_i64(&mut attributes, OUTPUT_TOKENS, self.data.output_tokens);
        push_i64(
            &mut attributes,
            REASONING_TOKENS,
            self.data.reasoning_tokens,
        );
        for attribute in &self.data.unknown_attributes {
            validate_unknown_key(&attribute.key)?;
            attributes.push(EncodedAttribute {
                key: attribute.key.clone(),
                value: attribute.value.clone(),
                required: false,
            });
        }
        if attributes.len() > MAX_ATTRIBUTES {
            return Err(Error::Limit {
                what: "exchange metadata attribute count",
                actual: attributes.len() as u64,
                limit: MAX_ATTRIBUTES as u64,
            });
        }
        for attribute in &attributes {
            validate_key_length(attribute.key.len(), limits)?;
            validate_value_length(attribute.value.len(), limits)?;
            if attribute.key.is_empty() {
                return Err(Error::Invalid("exchange metadata key is empty"));
            }
        }
        attributes.sort_by(|left, right| left.key.cmp(&right.key));
        if attributes.windows(2).any(|pair| pair[0].key == pair[1].key) {
            return Err(Error::Invalid("duplicate exchange metadata key"));
        }
        Ok(attributes)
    }
}

fn decode_known(data: &mut ExchangeMetadataData, key: &[u8], value: Vec<u8>) -> Result<bool> {
    macro_rules! bytes_field {
        ($key:expr, $field:ident) => {
            if key == $key {
                data.$field = Some(value);
                return Ok(true);
            }
        };
    }
    macro_rules! bool_field {
        ($key:expr, $field:ident) => {
            if key == $key {
                data.$field = decode_bool(&value)?;
                return Ok(true);
            }
        };
    }
    if key == TS_REQUEST_MS {
        data.ts_request_ms = Some(decode_i64(&value)?);
        return Ok(true);
    }
    if key == TS_RESPONSE_MS {
        data.ts_response_ms = Some(decode_i64(&value)?);
        return Ok(true);
    }
    bytes_field!(HARNESS, harness);
    bytes_field!(CLIENT_FORMAT, client_format);
    bytes_field!(UPSTREAM_FORMAT, upstream_format);
    bytes_field!(METHOD, method);
    bytes_field!(PATH, path);
    if key == STREAMED {
        data.streamed = Some(decode_bool(&value)?);
        return Ok(true);
    }
    if key == STATUS {
        data.status = Some(decode_i64(&value)?);
        return Ok(true);
    }
    if key == COST_USD_F64_BITS {
        data.cost_usd_bits = Some(decode_u64(&value)?);
        return Ok(true);
    }
    bytes_field!(BILLING_BUCKET, billing_bucket);
    bytes_field!(ERROR_KIND, error_kind);
    bytes_field!(ERROR_CODE, error_code);
    bool_field!(SUBSTITUTED, substituted);
    bytes_field!(ORIGINAL_MODEL, original_model);
    bytes_field!(SERVED_MODEL, served_model);
    bytes_field!(SUBSTITUTION_REASON, substitution_reason);
    bool_field!(INJECTED, injected);
    bytes_field!(FIXTURE_NAME, fixture_name);
    bytes_field!(ATTEMPTS_JSON, attempts_json);
    bytes_field!(ORIGINAL_ACCOUNT_ID, original_account_id);
    bytes_field!(SERVED_ACCOUNT_ID, served_account_id);
    bytes_field!(SUBSCRIPTION_IDENTITY, subscription_identity);
    bool_field!(VIA_DARIO, via_dario);
    bytes_field!(DARIO_GENERATION, dario_generation);
    bytes_field!(TAGS_JSON, tags_json);
    bytes_field!(CLIENT_IP, client_ip);
    bytes_field!(KEY_FINGERPRINT, key_fingerprint);
    bytes_field!(REASONING_EFFORT, reasoning_effort);
    if key == THINKING_BUDGET {
        data.thinking_budget = Some(decode_i64(&value)?);
        return Ok(true);
    }
    if key == CACHE_CREATION_TOKENS {
        data.cache_creation_tokens = Some(decode_i64(&value)?);
        return Ok(true);
    }
    if key == INPUT_TOKENS {
        data.input_tokens = Some(decode_i64(&value)?);
        return Ok(true);
    }
    if key == CACHED_INPUT_TOKENS {
        data.cached_input_tokens = Some(decode_i64(&value)?);
        return Ok(true);
    }
    if key == OUTPUT_TOKENS {
        data.output_tokens = Some(decode_i64(&value)?);
        return Ok(true);
    }
    if key == REASONING_TOKENS {
        data.reasoning_tokens = Some(decode_i64(&value)?);
        return Ok(true);
    }
    Ok(false)
}

fn push_bytes(attributes: &mut Vec<EncodedAttribute>, key: &[u8], value: Option<&[u8]>) {
    if let Some(value) = value {
        attributes.push(EncodedAttribute {
            key: key.to_vec(),
            value: value.to_vec(),
            required: false,
        });
    }
}

fn push_bool(attributes: &mut Vec<EncodedAttribute>, key: &[u8], value: Option<bool>) {
    if let Some(value) = value {
        attributes.push(EncodedAttribute {
            key: key.to_vec(),
            value: vec![u8::from(value)],
            required: false,
        });
    }
}

fn push_i64(attributes: &mut Vec<EncodedAttribute>, key: &[u8], value: Option<i64>) {
    if let Some(value) = value {
        attributes.push(EncodedAttribute {
            key: key.to_vec(),
            value: value.to_le_bytes().to_vec(),
            required: false,
        });
    }
}

fn push_u64(attributes: &mut Vec<EncodedAttribute>, key: &[u8], value: Option<u64>) {
    if let Some(value) = value {
        attributes.push(EncodedAttribute {
            key: key.to_vec(),
            value: value.to_le_bytes().to_vec(),
            required: false,
        });
    }
}

fn decode_bool(value: &[u8]) -> Result<bool> {
    match value {
        [0] => Ok(false),
        [1] => Ok(true),
        _ => Err(Error::Invalid("invalid exchange metadata boolean")),
    }
}

fn decode_i64(value: &[u8]) -> Result<i64> {
    let bytes: [u8; 8] = value
        .try_into()
        .map_err(|_| Error::Invalid("invalid exchange metadata i64"))?;
    Ok(i64::from_le_bytes(bytes))
}

fn decode_u64(value: &[u8]) -> Result<u64> {
    let bytes: [u8; 8] = value
        .try_into()
        .map_err(|_| Error::Invalid("invalid exchange metadata u64"))?;
    Ok(u64::from_le_bytes(bytes))
}

fn validate_key_length(length: usize, limits: &Limits) -> Result<()> {
    let limit = MAX_ATTRIBUTE_KEY_LENGTH.min(limits.max_identifier_length as usize);
    if length > limit {
        return Err(Error::Limit {
            what: "exchange metadata key",
            actual: length as u64,
            limit: limit as u64,
        });
    }
    Ok(())
}

fn validate_value_length(length: usize, limits: &Limits) -> Result<()> {
    if length > limits.max_field_length as usize {
        return Err(Error::Limit {
            what: "exchange metadata value",
            actual: length as u64,
            limit: limits.max_field_length as u64,
        });
    }
    Ok(())
}

fn validate_unknown_key(key: &[u8]) -> Result<()> {
    if key.is_empty()
        || !key[0].is_ascii_lowercase()
        || !key
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(byte))
    {
        return Err(Error::Invalid(
            "exchange metadata extension key is not canonical ASCII",
        ));
    }
    let canonical = std::str::from_utf8(key)
        .map_err(|_| Error::Invalid("exchange metadata extension key is not UTF-8"))?;
    let forbidden = canonical.split(['.', '_', '-']).any(|token| {
        matches!(
            token,
            "body"
                | "bodies"
                | "header"
                | "headers"
                | "manifest"
                | "manifests"
                | "chunk"
                | "chunks"
                | "ref"
                | "refs"
                | "path"
        )
    });
    if forbidden {
        return Err(Error::Invalid(
            "exchange metadata cannot contain body, header, manifest, or path extension keys",
        ));
    }
    Ok(())
}

fn read_u8(input: &mut Cursor<&[u8]>) -> Result<u8> {
    Ok(read_array::<1>(input)?[0])
}

fn read_u16(input: &mut Cursor<&[u8]>) -> Result<u16> {
    Ok(u16::from_le_bytes(read_array(input)?))
}

fn read_u32(input: &mut Cursor<&[u8]>) -> Result<u32> {
    Ok(u32::from_le_bytes(read_array(input)?))
}

fn read_array<const N: usize>(input: &mut Cursor<&[u8]>) -> Result<[u8; N]> {
    let mut bytes = [0; N];
    input
        .read_exact(&mut bytes)
        .map_err(|_| Error::Invalid("truncated exchange metadata record"))?;
    Ok(bytes)
}

fn read_bytes(input: &mut Cursor<&[u8]>, length: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0; length];
    input
        .read_exact(&mut bytes)
        .map_err(|_| Error::Invalid("truncated exchange metadata record"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_record(attributes: &[(&[u8], &[u8], u8)]) -> Vec<u8> {
        let mut out = vec![7; 32];
        out.extend_from_slice(PAYLOAD_MAGIC);
        out.extend_from_slice(&PAYLOAD_SCHEMA_V1.to_le_bytes());
        out.extend_from_slice(&(attributes.len() as u16).to_le_bytes());
        for (key, value, flags) in attributes {
            out.push(*flags);
            out.push(0);
            out.extend_from_slice(&(key.len() as u16).to_le_bytes());
            out.extend_from_slice(&(value.len() as u32).to_le_bytes());
            out.extend_from_slice(key);
            out.extend_from_slice(value);
        }
        out
    }

    #[test]
    fn unknown_optional_attributes_are_preserved_canonically() {
        let raw = raw_record(&[(b"future.attribute", b"future value", 0)]);
        let decoded = ExchangeMetadata::decode(&raw, &Limits::default()).unwrap();
        assert_eq!(
            decoded.data.unknown_attributes,
            vec![UnknownExchangeMetadataAttribute {
                key: b"future.attribute".to_vec(),
                value: b"future value".to_vec(),
            }]
        );
        assert_eq!(decoded.encode(&Limits::default()).unwrap(), raw);
    }

    #[test]
    fn unknown_required_attributes_are_rejected() {
        let raw = raw_record(&[(b"future.required", b"value", ATTRIBUTE_REQUIRED)]);
        assert!(matches!(
            ExchangeMetadata::decode(&raw, &Limits::default()),
            Err(Error::Unsupported(message)) if message.contains("future.required")
        ));
    }

    #[test]
    fn limits_and_canonical_order_are_enforced() {
        let limits = Limits {
            max_field_length: 3,
            ..Limits::default()
        };
        let value = ExchangeMetadata::new(
            ExchangeId([1; 32]),
            ExchangeMetadataData {
                harness: Some(b"four".to_vec()),
                ..ExchangeMetadataData::default()
            },
        );
        assert!(matches!(
            value.encode(&limits),
            Err(Error::Limit {
                what: "exchange metadata value",
                ..
            })
        ));

        let unsorted = raw_record(&[(b"z", b"1", 0), (b"a", b"2", 0)]);
        assert!(matches!(
            ExchangeMetadata::decode(&unsorted, &Limits::default()),
            Err(Error::Invalid(
                "exchange metadata keys are not strictly sorted"
            ))
        ));

        let forbidden = ExchangeMetadata::new(
            ExchangeId([2; 32]),
            ExchangeMetadataData {
                unknown_attributes: vec![UnknownExchangeMetadataAttribute {
                    key: b"x.request_body_path".to_vec(),
                    value: b"/tmp/body.gz".to_vec(),
                }],
                ..ExchangeMetadataData::default()
            },
        );
        assert!(matches!(
            forbidden.encode(&Limits::default()),
            Err(Error::Invalid(
                "exchange metadata cannot contain body, header, manifest, or path extension keys"
            ))
        ));

        let uppercase_bypass = ExchangeMetadata::new(
            ExchangeId([3; 32]),
            ExchangeMetadataData {
                unknown_attributes: vec![UnknownExchangeMetadataAttribute {
                    key: b"x.Future.Body".to_vec(),
                    value: b"bytes".to_vec(),
                }],
                ..ExchangeMetadataData::default()
            },
        );
        assert!(matches!(
            uppercase_bypass.encode(&Limits::default()),
            Err(Error::Invalid(
                "exchange metadata extension key is not canonical ASCII"
            ))
        ));

        let too_many = ExchangeMetadata::new(
            ExchangeId([4; 32]),
            ExchangeMetadataData {
                unknown_attributes: (0..=MAX_ATTRIBUTES)
                    .map(|index| UnknownExchangeMetadataAttribute {
                        key: format!("x.field.{index:03}").into_bytes(),
                        value: Vec::new(),
                    })
                    .collect(),
                ..ExchangeMetadataData::default()
            },
        );
        assert!(matches!(
            too_many.encode(&Limits::default()),
            Err(Error::Limit {
                what: "exchange metadata attribute count",
                ..
            })
        ));
    }
}
