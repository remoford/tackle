// What a message would cost at API list prices, from the usage Claude Code records in the
// transcript. Prices are dollars per million tokens, as listed on 2026-09-25; edit RATES
// when they change. Cache writes cost 1.25x input for the 5-minute cache and 2x for the
// 1-hour cache; fast mode doubles input and output.

use serde_json::Value;

struct Rates {
    input: f64,
    output: f64,
    cache_read: f64,
}

/// Model id prefix -> rates. The first matching prefix wins, so longer ids come first.
const RATES: &[(&str, Rates)] = &[
    ("claude-fable-5-1", Rates { input: 10.0, output: 50.0, cache_read: 0.25 }),
    ("claude-mythos-5-1", Rates { input: 10.0, output: 50.0, cache_read: 0.25 }),
    ("claude-fable-5", Rates { input: 10.0, output: 50.0, cache_read: 1.00 }),
    ("claude-opus-5-5", Rates { input: 4.0, output: 20.0, cache_read: 0.20 }),
    ("claude-opus-5", Rates { input: 5.0, output: 25.0, cache_read: 0.50 }),
    ("claude-opus-4", Rates { input: 5.0, output: 25.0, cache_read: 0.50 }),
    ("claude-sonnet-5-5", Rates { input: 2.0, output: 10.0, cache_read: 0.20 }),
    ("claude-sonnet-5", Rates { input: 2.0, output: 10.0, cache_read: 0.20 }),
    ("claude-sonnet-4", Rates { input: 3.0, output: 15.0, cache_read: 0.30 }),
    ("claude-haiku-4", Rates { input: 1.0, output: 5.0, cache_read: 0.10 }),
];

/// The API-price cost of one message, or None for a model not in the table.
pub fn cost(model: &str, usage: &Value) -> Option<f64> {
    let r = &RATES.iter().find(|(prefix, _)| model.starts_with(prefix))?.1;
    let n = |v: &Value| v.as_u64().unwrap_or(0) as f64 / 1e6;
    let fast = if usage["speed"] == "fast" { 2.0 } else { 1.0 };
    let (w1h, w5m) = match usage["cache_creation"].as_object() {
        Some(c) => (n(&c["ephemeral_1h_input_tokens"]), n(&c["ephemeral_5m_input_tokens"])),
        None => (0.0, n(&usage["cache_creation_input_tokens"])),
    };
    Some(
        n(&usage["input_tokens"]) * r.input * fast
            + n(&usage["output_tokens"]) * r.output * fast
            + n(&usage["cache_read_input_tokens"]) * r.cache_read
            + w1h * r.input * 2.0
            + w5m * r.input * 1.25,
    )
}
