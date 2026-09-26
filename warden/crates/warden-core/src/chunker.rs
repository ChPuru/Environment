use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::io::Read;

/// FastCDC (v2020) content-defined chunking: boundaries follow content, so an
/// edit re-chunks only its neighborhood and dedup survives insertions. Every
/// chunk is <= max; every chunk except possibly the stream's last is >= min.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkerParams {
    pub min: u32,
    pub avg: u32,
    pub max: u32,
}

impl ChunkerParams {
    pub const DEFAULT: ChunkerParams = ChunkerParams {
        min: 512 * 1024,
        avg: 1024 * 1024,
        max: 8 * 1024 * 1024,
    };
    /// Tiny bounds so property tests exercise many chunks on small inputs.
    pub const TINY_FOR_TESTS: ChunkerParams = ChunkerParams {
        min: 64,
        avg: 256,
        max: 1024,
    };
}

/// Stream `r` as content-defined chunks, in order, as owned byte vectors.
/// Contract (I2): concatenating every yielded chunk reproduces the input
/// byte stream exactly.
pub fn chunk_reader<R: Read>(r: R, p: ChunkerParams) -> impl Iterator<Item = Result<Vec<u8>>> {
    fastcdc::v2020::StreamCDC::new(r, p.min, p.avg, p.max).map(|res| {
        res.map(|c| c.data)
            .map_err(|e| crate::error::Error::Format(format!("chunker: {e:?}")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn collect(data: &[u8], p: ChunkerParams) -> Vec<Vec<u8>> {
        chunk_reader(data, p)
            .collect::<Result<Vec<_>>>()
            .expect("chunking in-memory data cannot fail")
    }

    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(collect(&[], ChunkerParams::TINY_FOR_TESTS).is_empty());
    }

    #[test]
    fn chunking_is_deterministic() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i * 31 % 251) as u8).collect();
        let a = collect(&data, ChunkerParams::TINY_FOR_TESTS);
        let b = collect(&data, ChunkerParams::TINY_FOR_TESTS);
        assert_eq!(a, b);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn concat_of_chunks_is_identity_and_bounds_hold(
            data in proptest::collection::vec(any::<u8>(), 0..8192)
        ) {
            let p = ChunkerParams::TINY_FOR_TESTS;
            let chunks = collect(&data, p);
            let joined: Vec<u8> = chunks.iter().flatten().copied().collect();
            prop_assert_eq!(&joined, &data, "I2: concat(chunks) != input");
            for (i, c) in chunks.iter().enumerate() {
                prop_assert!(c.len() <= p.max as usize, "chunk {} over max", i);
                prop_assert!(!c.is_empty(), "empty chunk {}", i);
                if i + 1 < chunks.len() {
                    prop_assert!(c.len() >= p.min as usize, "non-final chunk {} under min", i);
                }
            }
        }
    }
}
