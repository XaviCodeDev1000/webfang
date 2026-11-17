//! Explicit-`null` rejection deserializers for optional config fields.
//!
//! With plain `#[serde(default)]`, `Option<T>` maps an explicit `null` to
//! `None` — a silent adopt. Config-file `null`s are typos, and typos must
//! fail loudly (#1462): routing through `deserialize_option` with a visitor
//! that errors on `None` keeps absent → `None` (serde never calls the
//! deserializer for missing fields) while failing loudly on `null`.
//!
//! One shared scan (`no_null_option`), one named wrapper per field type:
//! `serde`'s `deserialize_with = "path"` only accepts monomorphic functions,
//! so each optional field gets a wrapper carrying its own Spanish message.
//! The wrapper bodies are four lines against one core — keeping them inline
//! per module would duplicate the visitor, and a jscpd clone against a
//! descending ratchet is the worse ratchet.

use std::marker::PhantomData;

/// The shared explicit-`null` rejection scan: absent → `None`, `null` →
/// error carrying `msg`, present → `T` deserialized as usual.
fn no_null_option<'de, D, T>(deserializer: D, msg: &'static str) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    struct NoNull<T>(&'static str, PhantomData<T>);

    impl<'de, T: serde::Deserialize<'de>> serde::de::Visitor<'de> for NoNull<T> {
        type Value = Option<T>;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Err(E::custom(self.0))
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Err(E::custom(self.0))
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            <T as serde::Deserialize<'de>>::deserialize(deserializer).map(Some)
        }
    }

    deserializer.deserialize_option(NoNull::<T>(msg, PhantomData))
}

const DIM_NO_NULL_MSG: &str =
    "embedding_dim: null explícito no permitido; omití el campo para adoptar la dimensión remota";

/// Reject an explicit `null` for `embedding_dim` while keeping the
/// absent-means-`None` default (#1462).
pub(crate) fn deserialize_dim_no_null<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    no_null_option::<D, usize>(deserializer, DIM_NO_NULL_MSG)
}

const PROXY_NO_NULL_MSG: &str =
    "proxy: null explícito no permitido; omití el campo, o referenciá un id de [[proxies]] o escribí la tabla inline";

/// Reject an explicit `null` for the per-provider `proxy` reference while
/// keeping the absent-means-direct-egress default (ADR-0021).
pub(crate) fn deserialize_proxy_spec_no_null<'de, D>(
    deserializer: D,
) -> Result<Option<crate::domain::proxy_config::ProxySpec>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    no_null_option::<D, crate::domain::proxy_config::ProxySpec>(deserializer, PROXY_NO_NULL_MSG)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Deserialize)]
    struct Dim {
        #[serde(default, deserialize_with = "deserialize_dim_no_null")]
        dim: Option<usize>,
    }

    #[derive(Debug, Deserialize)]
    struct Probe {
        #[serde(default, deserialize_with = "deserialize_proxy_spec_no_null")]
        spec: Option<crate::domain::proxy_config::ProxySpec>,
    }

    #[test]
    fn absent_field_deserializes_to_none() {
        assert_eq!(
            serde_json::from_str::<Dim>("{}").expect("absent"),
            Dim { dim: None }
        );
    }

    #[test]
    fn present_value_deserializes_to_some() {
        let parsed = serde_json::from_str::<Dim>(r#"{"dim": 1536}"#).expect("present");
        assert_eq!(parsed, Dim { dim: Some(1536) });
    }

    #[test]
    fn explicit_null_is_a_spanish_error_naming_the_field() {
        let err = serde_json::from_str::<Dim>(r#"{"dim": null}"#).expect_err("null is a typo");
        assert!(
            err.to_string().contains("embedding_dim"),
            "message must name the field, got: {err}"
        );
        let err = serde_json::from_str::<Probe>(r#"{"spec": null}"#).expect_err("null is a typo");
        assert!(
            err.to_string().contains("proxy"),
            "message must name the field, got: {err}"
        );
    }

    #[test]
    fn present_proxy_spec_parses_through_the_wrapper() {
        let parsed = serde_json::from_str::<Probe>(r#"{"spec": "corp-egress"}"#)
            .expect("named spec parses through the wrapper");
        assert!(matches!(
            parsed.spec,
            Some(crate::domain::proxy_config::ProxySpec::Named(_))
        ));
    }
}
