//! The plugin protocol (M0.6, DEC-073): what the `caravel` CLI asks of a
//! template's binary, under its hidden `plugin` subcommand. Each command
//! prints one JSON document on stdout.
//!
//! - `plugin info`: the protocol, the template, this binary's release, the
//!   engine's file in a release and the token decimals the template needs.
//! - `plugin example`: a lane file to start from, with the placeholders that
//!   `caravel init` fills: `{{name}}`, `{{port}}`, `{{id.<role>}}` (a Stellar
//!   CLI identity) and `{{g.<role>}}` (that identity's public key).
//! - `plugin body [--decimals D] <body…>`: a transaction body's kind and
//!   bytes, in the template's own `tx` syntax. `caravel` puts it in an
//!   envelope (spec §9.2), signs it and submits it, so no key reaches the
//!   template's binary.
//!
//! With `genesis` (the lane id and hashes), `replay` and `export-proofs`,
//! that is all `caravel` needs from a template.

use anyhow::{anyhow, bail, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use crate::app::NodeApp;

/// Bumped when a reply changes shape; `caravel` refuses another.
pub const PROTOCOL: u32 = 1;

#[derive(Subcommand, Debug)]
pub enum Command {
    /// The template and this binary, as JSON.
    Info,
    /// A lane file to start from, with placeholders for `caravel init`.
    Example,
    /// A transaction body's kind and bytes (hex), as JSON.
    Body(BodyArgs),
}

#[derive(Args, Debug)]
pub struct BodyArgs {
    /// Read token amounts in token units with up to this many decimals
    /// ("12.5"); without it they are base units.
    #[arg(long)]
    pub decimals: Option<u32>,
    /// The body, as `tx` takes it (e.g. `transfer --to G… --amount 5`).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    pub args: Vec<String>,
}

/// `plugin info`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    pub protocol: u32,
    pub template: String,
    pub version: String,
    /// The commit CI built this binary from, if it did.
    pub commit: Option<String>,
    /// The engine's file under a release's `contracts/`.
    pub engine_file: String,
    /// The decimals the template needs of the settlement token, if any.
    pub token_decimals: Option<u32>,
}

pub fn info<A: NodeApp>(app: &A) -> Info {
    Info {
        protocol: PROTOCOL,
        template: A::TEMPLATE.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        commit: option_env!("CARAVEL_COMMIT").map(String::from),
        engine_file: format!("{}_engine.wasm", A::TEMPLATE),
        token_decimals: app.token_decimals(),
    }
}

/// `plugin body`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Body {
    pub kind: u8,
    /// The body's bytes, hex.
    pub body: String,
}

/// Runs one plugin command for `app`. `example` is the template's lane file
/// scaffold; `body` turns the body's arguments (and the decimals for token
/// amounts) into a kind and bytes.
pub fn run<A: NodeApp>(
    app: &A,
    cmd: Command,
    example: &str,
    body: impl FnOnce(&[String], Option<u32>) -> Result<(u8, Vec<u8>)>,
) -> Result<()> {
    match cmd {
        Command::Info => println!("{}", serde_json::to_string(&info(app))?),
        Command::Example => print!("{example}"),
        Command::Body(a) => {
            let (kind, bytes) = body(&a.args, a.decimals)?;
            let reply = Body {
                kind,
                body: caravel_runtime::sequencer::hex(&bytes),
            };
            println!("{}", serde_json::to_string(&reply)?);
        }
    }
    Ok(())
}

/// One placeholder of an example lane file.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Placeholder<'a> {
    Name,
    Port,
    /// `{{id.<role>}}`: the role's Stellar CLI identity.
    Id(&'a str),
    /// `{{g.<role>}}`: that identity's public key.
    G(&'a str),
}

/// Every placeholder in `text`, with its byte range.
fn placeholders(text: &str) -> Result<Vec<(std::ops::Range<usize>, Placeholder<'_>)>> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(open) = text[at..].find("{{").map(|i| at + i) {
        let close = text[open..]
            .find("}}")
            .map(|i| open + i)
            .ok_or_else(|| anyhow!("an unclosed {{{{ at byte {open}"))?;
        let inner = &text[open + 2..close];
        let role_ok = |r: &str| {
            !r.is_empty()
                && r.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        let p = match inner {
            "name" => Placeholder::Name,
            "port" => Placeholder::Port,
            _ => match inner.split_once('.') {
                Some(("id", r)) if role_ok(r) => Placeholder::Id(r),
                Some(("g", r)) if role_ok(r) => Placeholder::G(r),
                _ => bail!("unknown placeholder {{{{{inner}}}}}"),
            },
        };
        out.push((open..close + 2, p));
        at = close + 2;
    }
    Ok(out)
}

/// The roles an example names, through `{{id.<role>}}` or `{{g.<role>}}`.
pub fn example_roles(text: &str) -> Result<std::collections::BTreeSet<String>> {
    Ok(placeholders(text)?
        .into_iter()
        .filter_map(|(_, p)| match p {
            Placeholder::Id(r) | Placeholder::G(r) => Some(r.to_string()),
            _ => None,
        })
        .collect())
}

/// An example with its placeholders filled: the lane's name, the sequencer's
/// port, and for each role its identity and public key (`G…`).
pub fn fill_example(
    text: &str,
    name: &str,
    port: u16,
    ids: &std::collections::BTreeMap<String, (String, String)>,
) -> Result<String> {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (range, p) in placeholders(text)? {
        out.push_str(&text[at..range.start]);
        let role = |r: &str| {
            ids.get(r)
                .ok_or_else(|| anyhow!("no identity for the role {r:?}"))
        };
        match p {
            Placeholder::Name => out.push_str(name),
            Placeholder::Port => out.push_str(&port.to_string()),
            Placeholder::Id(r) => out.push_str(&role(r)?.0),
            Placeholder::G(r) => out.push_str(&role(r)?.1),
        }
        at = range.end;
    }
    out.push_str(&text[at..]);
    Ok(out)
}

/// A token amount: base units ("1000000") without `decimals`, or token units
/// with at most `decimals` digits after the point ("1.5") with them. Integer
/// arithmetic only.
pub fn parse_amount(s: &str, decimals: Option<u32>) -> Result<i128> {
    let bad = || anyhow!("{s:?} is not an amount");
    let Some(d) = decimals else {
        return s.parse().map_err(|_| bad());
    };
    let (neg, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (whole, frac) = match digits.split_once('.') {
        Some((_, "")) => return Err(bad()),
        Some(parts) => parts,
        None => (digits, ""),
    };
    let all_digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if whole.is_empty() || !all_digits(whole) || !all_digits(frac) {
        return Err(bad());
    }
    if frac.len() > d as usize {
        bail!("{s:?} has more than {d} decimals");
    }
    let scale = 10i128.checked_pow(d).ok_or_else(bad)?;
    let whole: i128 = whole.parse().map_err(|_| bad())?;
    let frac_units: i128 = if frac.is_empty() {
        0
    } else {
        let pad = 10i128.pow(d - frac.len() as u32);
        frac.parse::<i128>().map_err(|_| bad())? * pad
    };
    let v = whole
        .checked_mul(scale)
        .and_then(|w| w.checked_add(frac_units))
        .ok_or_else(|| anyhow!("{s:?} is too large"))?;
    Ok(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{example_roles, fill_example, parse_amount};

    #[test]
    fn examples() {
        let text = "name = \"{{name}}\"\nport = {{port}}\nkey = \"{{id.v1}}\" # {{g.v1}}\nt = \"{{g.treasury}}\"\n";
        let roles: Vec<_> = example_roles(text).unwrap().into_iter().collect();
        assert_eq!(roles, ["treasury", "v1"]);
        let ids = BTreeMap::from([
            ("v1".to_string(), ("me-v1".to_string(), "GV1".to_string())),
            (
                "treasury".to_string(),
                ("me-t".to_string(), "GT".to_string()),
            ),
        ]);
        assert_eq!(
            fill_example(text, "acme", 18080, &ids).unwrap(),
            "name = \"acme\"\nport = 18080\nkey = \"me-v1\" # GV1\nt = \"GT\"\n"
        );
        assert!(fill_example(text, "acme", 1, &BTreeMap::new()).is_err());
        for bad in ["{{nme}}", "{{id.}}", "{{id.V1}}", "{{x.v1}}", "{{name"] {
            assert!(example_roles(bad).is_err(), "{bad}");
        }
        assert_eq!(
            fill_example("no placeholders", "a", 1, &ids).unwrap(),
            "no placeholders"
        );
    }

    #[test]
    fn amounts() {
        assert_eq!(parse_amount("1000000", None).unwrap(), 1_000_000);
        assert_eq!(parse_amount("5", Some(7)).unwrap(), 50_000_000);
        assert_eq!(parse_amount("12.5", Some(7)).unwrap(), 125_000_000);
        assert_eq!(parse_amount("0.0000001", Some(7)).unwrap(), 1);
        assert_eq!(parse_amount("-1.5", Some(2)).unwrap(), -150);
        assert_eq!(parse_amount("3", Some(0)).unwrap(), 3);
        for bad in ["", ".5", "1.", "1.2.3", "1e5", "abc", "1,5", "+1"] {
            assert!(parse_amount(bad, Some(7)).is_err(), "{bad:?}");
        }
        assert!(parse_amount("1.00000001", Some(7)).is_err());
        assert!(parse_amount("1.5", None).is_err());
        assert!(parse_amount("1", Some(40)).is_err());
        assert!(parse_amount(&"9".repeat(40), Some(7)).is_err());
    }
}
