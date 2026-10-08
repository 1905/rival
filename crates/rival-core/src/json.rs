//! JSON reading for rival's own files (sessions, queue tickets, the update
//! cache) and for reviewer payloads, plus the time format they share.
//!
//! Files are written with plain `serde_json::to_vec_pretty` / `to_vec` and
//! read with [`decode`] into structs marked `#[serde(default)]`, so a missing
//! key gives the default and unknown keys are ignored. Key names match
//! exactly. A field marked `deserialize_with = "json::nullable"` also reads
//! `null` as its default, as older readers did.

use std::fmt;
use std::marker::PhantomData;

use chrono::{DateTime, FixedOffset, SecondsFormat};
use serde::de::value::MapAccessDeserializer;
use serde::de::{DeserializeOwned, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serializer};

/// Decodes a JSON object into `T`. Anything else (an array, `null`, a
/// number) is an error.
pub fn decode<T: DeserializeOwned>(data: &[u8]) -> serde_json::Result<T> {
    serde_json::from_slice::<Object<T>>(data).map(|o| o.0)
}

/// A `T` that only a JSON object decodes into. A serde-derived struct also
/// accepts an array of its field values; this wrapper does not.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Object<T>(pub T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct ObjectVisitor<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
            type Value = Object<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Object<T>, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(Object)
            }
        }
        d.deserialize_map(ObjectVisitor(PhantomData))
    }
}

/// Reads `null` as the field's default value.
pub fn nullable<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// The text of a stored time: RFC 3339, `Z` for UTC, and 0, 3, 6 or 9
/// fraction digits.
pub fn format_time(t: &DateTime<FixedOffset>) -> String {
    t.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// `0001-01-01T00:00:00Z` in Unix seconds. Older releases wrote this time for "unset",
/// so their files carry it.
const OLD_UNSET_TIME_SECS: i64 = -62_135_596_800;

/// Parses a stored time. The old "unset" time `0001-01-01T00:00:00Z` gives
/// `None`.
pub fn parse_time(s: &str) -> Result<Option<DateTime<FixedOffset>>, chrono::ParseError> {
    let t = DateTime::parse_from_rfc3339(s)?;
    let unset = t.timestamp() == OLD_UNSET_TIME_SECS && t.timestamp_subsec_nanos() == 0;
    Ok((!unset).then_some(t))
}

/// Serde adapter for an optional time field. Use it with
/// `#[serde(default, skip_serializing_if = "Option::is_none", with = "json::opt_time")]`:
/// `None` is not written, and a missing key, `null` or the old unset time
/// read as `None`.
pub mod opt_time {
    use super::*;

    pub fn serialize<S: Serializer>(
        t: &Option<DateTime<FixedOffset>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => s.serialize_str(&format_time(t)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<DateTime<FixedOffset>>, D::Error> {
        match Option::<String>::deserialize(d)? {
            None => Ok(None),
            Some(text) => parse_time(&text)
                .map_err(|e| serde::de::Error::custom(format!("parse time {text:?}: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[derive(Debug, Default, PartialEq, Deserialize, serde::Serialize)]
    #[serde(default)]
    struct Rec {
        #[serde(deserialize_with = "nullable")]
        name: String,
        #[serde(deserialize_with = "nullable")]
        n: i64,
        #[serde(skip_serializing_if = "Option::is_none", with = "opt_time")]
        at: Option<DateTime<FixedOffset>>,
    }

    fn decode(data: &str) -> serde_json::Result<Rec> {
        super::decode(data.as_bytes())
    }

    fn rec(data: &str) -> Rec {
        decode(data).unwrap()
    }

    #[test]
    fn null_reads_as_the_default_and_unknown_keys_are_ignored() {
        assert_eq!(
            rec(r#"{"name":null,"n":null,"at":null,"x":{"y":null},"big":1e999999}"#),
            Rec::default()
        );
        assert_eq!(rec("{}"), Rec::default());
        assert_eq!(rec(r#"{"name":"a","other":[1,null]}"#).name, "a");
    }

    #[test]
    fn keys_match_exactly_and_a_duplicate_is_an_error() {
        assert_eq!(rec(r#"{"Name":"a"}"#).name, "");
        assert!(decode(r#"{"name":"a","name":"b"}"#).is_err());
    }

    #[test]
    fn wrong_types_and_bad_times_are_errors() {
        for doc in [
            r#"{"n":"1"}"#,
            r#"{"n":1.5}"#,
            r#"{"at":5}"#,
            r#"{"at":"nope"}"#,
            "[]",
            r#"["a"]"#,
            "null",
            "5",
            "{",
        ] {
            assert!(decode(doc).is_err(), "{doc}");
        }
    }

    #[test]
    fn old_unset_time_and_omitted_key_read_as_none() {
        assert_eq!(rec(r#"{"at":"0001-01-01T00:00:00Z"}"#).at, None);
        assert_eq!(rec(r#"{"at":"0001-01-01T00:00:00+00:00"}"#).at, None);
        assert_eq!(rec("{}").at, None);
        let t = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(1, 1, 1, 0, 0, 1)
            .unwrap();
        assert_eq!(rec(r#"{"at":"0001-01-01T00:00:01Z"}"#).at, Some(t));
    }

    #[test]
    fn times_write_rfc3339_and_read_back_with_their_offset() {
        let t = FixedOffset::east_opt(-(3 * 3600 + 1800))
            .unwrap()
            .with_ymd_and_hms(2026, 9, 30, 23, 59, 58)
            .unwrap()
            + chrono::TimeDelta::milliseconds(120);
        let r = Rec {
            at: Some(t),
            ..Rec::default()
        };
        let data = serde_json::to_string(&r).unwrap();
        assert_eq!(
            data,
            r#"{"name":"","n":0,"at":"2026-09-30T23:59:58.120-03:30"}"#
        );
        let back = rec(&data);
        assert_eq!(back, r);
        assert_eq!(back.at.unwrap().offset(), t.offset());
        assert_eq!(
            format_time(
                &FixedOffset::east_opt(0)
                    .unwrap()
                    .with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
                    .unwrap()
            ),
            "2026-01-02T03:04:05Z"
        );
        // A trimmed fraction reads the same.
        assert_eq!(rec(r#"{"at":"2026-09-30T23:59:58.12-03:30"}"#).at, Some(t));
        // Unset is not written.
        assert_eq!(
            serde_json::to_string(&Rec::default()).unwrap(),
            r#"{"name":"","n":0}"#
        );
    }

    #[test]
    fn writers_do_not_escape_html_characters() {
        let r = Rec {
            name: "a<b>&c\u{2028}".into(),
            ..Rec::default()
        };
        assert_eq!(
            serde_json::to_string(&r).unwrap(),
            "{\"name\":\"a<b>&c\u{2028}\",\"n\":0}"
        );
    }
}
