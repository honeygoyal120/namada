use std::hash::Hash;

mod generated;
pub use generated::*;

// TODO change timestamp type to chrono (support for serde) or std::time
#[allow(clippy::derive_hash_xor_eq)]
impl Hash for types::Intent {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.data.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use prost::Message;
    use types::Tx;

    use super::*;

    #[test]
    fn encoding_round_trip() {
        let tx = types::Tx {
            code: "wasm code".as_bytes().to_owned(),
            data: Some("arbitrary data".as_bytes().to_owned()),
        };
        let mut tx_bytes = vec![];
        tx.encode(&mut tx_bytes).unwrap();
        let tx_hex = hex::encode(tx_bytes);
        let tx_from_hex = hex::decode(tx_hex).unwrap();
        let tx_from_bytes = Tx::decode(&tx_from_hex[..]).unwrap();
        assert_eq!(tx, tx_from_bytes);
    }
}