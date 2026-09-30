//! The engine contract's `step` return value (spec §12.1):
//! `"CVSTEP01" (8) · state_len u32 · state · receipts_len u32 · receipts`.

use alloc::vec::Vec;

use crate::codec::{DecodeError, EncodeError, Reader, Writer};

pub const STEP_MAGIC: &[u8; 8] = b"CVSTEP01";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepEnvelope {
    /// `StateV1` bytes after the block.
    pub state: Vec<u8>,
    /// `CVRCPT01` receipts bytes.
    pub receipts: Vec<u8>,
}

impl StepEnvelope {
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut w = Writer::with_capacity(16 + self.state.len() + self.receipts.len());
        w.bytes(STEP_MAGIC);
        w.count_u32(self.state.len())?;
        w.bytes(&self.state);
        w.count_u32(self.receipts.len())?;
        w.bytes(&self.receipts);
        Ok(w.into_vec())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(bytes);
        r.magic(STEP_MAGIC)?;
        let n = usize::try_from(r.u32()?).map_err(|_| DecodeError::UnexpectedEnd)?;
        let state = r.take(n)?.to_vec();
        let n = usize::try_from(r.u32()?).map_err(|_| DecodeError::UnexpectedEnd)?;
        let receipts = r.take(n)?.to_vec();
        r.finish()?;
        Ok(Self { state, receipts })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_strictness() {
        let env = StepEnvelope {
            state: alloc::vec![1, 2, 3],
            receipts: alloc::vec![4],
        };
        let bytes = env.encode().unwrap();
        assert_eq!(bytes.len(), 8 + 4 + 3 + 4 + 1);
        assert_eq!(StepEnvelope::decode(&bytes), Ok(env));
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert_eq!(
            StepEnvelope::decode(&trailing),
            Err(DecodeError::TrailingBytes)
        );
        assert_eq!(
            StepEnvelope::decode(&bytes[..bytes.len() - 1]),
            Err(DecodeError::UnexpectedEnd)
        );
    }
}
