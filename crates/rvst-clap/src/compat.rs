//! Explicit legacy CLAP wire policy. Internal values always remain plain-domain.
use rvst_core::{ParameterSpec, Plugin};
pub fn maximum<P: Plugin>(p: &ParameterSpec) -> f64 {
    if P::adapter_compatibility().normalized_clap_parameters {
        p.step
            .map_or(1., |step| ((p.max - p.min) / step).round().max(1.))
    } else {
        p.max
    }
}
pub fn to_host<P: Plugin>(p: &ParameterSpec, plain: f64) -> f64 {
    if P::adapter_compatibility().normalized_clap_parameters {
        p.normalize(plain) * maximum::<P>(p)
    } else {
        p.clamp(plain)
    }
}
pub fn from_host<P: Plugin>(p: &ParameterSpec, value: f64) -> f64 {
    if P::adapter_compatibility().normalized_clap_parameters {
        p.denormalize(value / maximum::<P>(p))
    } else {
        p.clamp(value)
    }
}
pub fn modulation<P: Plugin>(p: &ParameterSpec, amount: f64) -> f64 {
    if P::adapter_compatibility().normalized_clap_parameters {
        amount / maximum::<P>(p) * (p.max - p.min)
    } else {
        amount
    }
}
pub fn encode_state<P: Plugin>(bytes: Vec<u8>) -> Vec<u8> {
    if !P::adapter_compatibility().clap_state_length_prefix {
        return bytes;
    }
    let mut out = Vec::with_capacity(bytes.len() + 8);
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(&bytes);
    out
}
pub fn decode_state<P: Plugin>(bytes: &[u8]) -> Option<&[u8]> {
    if !P::adapter_compatibility().clap_state_length_prefix {
        return Some(bytes);
    }
    let (length, body) = bytes.split_at_checked(8)?;
    (u64::from_le_bytes(length.try_into().ok()?) == body.len() as u64).then_some(body)
}
pub fn input_count<P: Plugin>(config: u32) -> usize {
    P::descriptor()
        .inputs
        .iter()
        .filter(|b| config == 0 || !b.auxiliary)
        .count()
}
