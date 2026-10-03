//! Contract addresses known before anything is sent. A Soroban contract's ID
//! is `H(HashIdPreimage::ContractId{network_id, Address{deployer, salt}})`: it
//! depends on the network, the deploying account and a salt, and not on the
//! Wasm or the constructor arguments. The deploy tool fixes the salt from the
//! lane, so a plan can name the settlement contract, and running apply again
//! finds the same one.

use caravel_runtime::checkpoint::{network_id, sha256};
use stellar_xdr::{
    AccountId, AlphaNum12, AlphaNum4, Asset, AssetCode12, AssetCode4, ContractIdPreimage,
    ContractIdPreimageFromAddress, Hash, HashIdPreimage, HashIdPreimageContractId, Limits,
    PublicKey, ScAddress, Uint256, WriteXdr,
};

/// Tag of the settlement salt.
pub const SETTLEMENT_SALT_TAG: &[u8] = b"caravel/settlement";

/// `H("caravel/settlement" ‖ lane_id)`: one settlement contract per lane and
/// admin on a network.
pub fn settlement_salt(lane_id: &[u8; 32]) -> [u8; 32] {
    sha256(&[SETTLEMENT_SALT_TAG, lane_id.as_slice()].concat())
}

/// The ID of the contract `deployer` (an ed25519 account) creates with `salt`
/// on the network with `passphrase`, as `stellar contract id wasm` gives it.
pub fn contract_id(passphrase: &str, deployer: &[u8; 32], salt: &[u8; 32]) -> [u8; 32] {
    id_of(
        passphrase,
        ContractIdPreimage::Address(ContractIdPreimageFromAddress {
            address: ScAddress::Account(account(deployer)),
            salt: Uint256(*salt),
        }),
    )
}

/// The Stellar Asset Contract of `code:issuer` (a 1–12 character code), as
/// `stellar contract id asset` gives it. A local lane's test token is
/// `<code>:<admin>`.
pub fn asset_contract_id(passphrase: &str, code: &str, issuer: &[u8; 32]) -> [u8; 32] {
    let asset = if code.len() <= 4 {
        let mut c = [0u8; 4];
        c[..code.len()].copy_from_slice(code.as_bytes());
        Asset::CreditAlphanum4(AlphaNum4 {
            asset_code: AssetCode4(c),
            issuer: account(issuer),
        })
    } else {
        let mut c = [0u8; 12];
        c[..code.len()].copy_from_slice(code.as_bytes());
        Asset::CreditAlphanum12(AlphaNum12 {
            asset_code: AssetCode12(c),
            issuer: account(issuer),
        })
    };
    id_of(passphrase, ContractIdPreimage::Asset(asset))
}

fn account(key: &[u8; 32]) -> AccountId {
    AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(*key)))
}

fn id_of(passphrase: &str, contract_id_preimage: ContractIdPreimage) -> [u8; 32] {
    let preimage = HashIdPreimage::ContractId(HashIdPreimageContractId {
        network_id: Hash(network_id(passphrase)),
        contract_id_preimage,
    });
    sha256(
        &preimage
            .to_xdr(Limits::none())
            .expect("a contract id preimage encodes"),
    )
}

pub fn strkey(contract: &[u8; 32]) -> String {
    stellar_strkey::Contract(*contract)
        .to_string()
        .as_str()
        .to_string()
}

/// A `C…` contract address, if `s` is one.
pub fn parse_contract(s: &str) -> Option<[u8; 32]> {
    stellar_strkey::Contract::from_string(s).ok().map(|c| c.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `stellar contract id wasm --salt 0101…01 --source-account GCQJ…B64N
    /// --network-passphrase "Test SDF Network ; September 2015"` (CLI 28.1.0,
    /// 2026-09-30).
    #[test]
    fn matches_the_stellar_cli() {
        let deployer = stellar_strkey::ed25519::PublicKey::from_string(
            "GCQJVJPUPJTVTABP7FK7RXBNFIKKLSM5EO7JP6DECJ77SOBUKWSPB64N",
        )
        .unwrap()
        .0;
        let id = contract_id("Test SDF Network ; September 2015", &deployer, &[1; 32]);
        assert_eq!(
            strkey(&id),
            "CAHCJFCZLOHU7K4CUOA7D45TVMUV7AMFEH7AHDJSBFL2YCEQH2LZLENB"
        );
        // Another network, another address.
        let local = contract_id("Standalone Network ; February 2017", &deployer, &[1; 32]);
        assert_ne!(local, id);
    }

    /// `stellar contract id asset --asset USDC:<issuer>` (CLI 28.1.0,
    /// 2026-09-30). The testnet one is Circle's USDC in versions.json.
    #[test]
    fn asset_contracts_match_the_stellar_cli() {
        let g = |s: &str| {
            stellar_strkey::ed25519::PublicKey::from_string(s)
                .unwrap()
                .0
        };
        let circle = asset_contract_id(
            "Test SDF Network ; September 2015",
            "USDC",
            &g("GBBD47IF6LWK7P7MDEVSCWR7DPUWV3NY3DTQEVFL4NAT4AQH3ZLLFLA5"),
        );
        assert_eq!(strkey(&circle), crate::versions::testnet_usdc());
        let local = asset_contract_id(
            "Standalone Network ; February 2017",
            "USDC",
            &g("GCQJVJPUPJTVTABP7FK7RXBNFIKKLSM5EO7JP6DECJ77SOBUKWSPB64N"),
        );
        assert_eq!(
            strkey(&local),
            "CANCYZ63P4PU3FM34UTM74ZXJ7GMZQHMPTMHIEWE7DPVKJVQLUTI6OJG"
        );
        // A 5-12 character code is an AlphaNum12 asset (CLI 28.1.0 vector).
        let long = asset_contract_id(
            "Test SDF Network ; September 2015",
            "LONGERCODE12",
            &g("GCQJVJPUPJTVTABP7FK7RXBNFIKKLSM5EO7JP6DECJ77SOBUKWSPB64N"),
        );
        assert_eq!(
            strkey(&long),
            "CCOTSX4E6KOC2DTSJDIAQECEFGO6P35XDX4LKCMA7OHW6SIOU7GHPYKH"
        );
    }

    #[test]
    fn the_salt_is_per_lane() {
        assert_ne!(settlement_salt(&[1; 32]), settlement_salt(&[2; 32]));
        assert_eq!(
            settlement_salt(&[1; 32]),
            sha256(&[b"caravel/settlement".as_slice(), &[1; 32]].concat())
        );
    }
}
