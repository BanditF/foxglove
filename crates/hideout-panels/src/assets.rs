// SPDX-License-Identifier: MIT
//! `ui.asset.put` (§5.3): an image uploaded once, keyed by its SHA-256 hash (64 hex digits), at
//! most 256 KiB decoded. Nodes refer to it by hash. The store is bounded: past
//! [`MAX_TOTAL_BYTES`] the oldest assets go first.

use base64::Engine;
use hideout_contract::error::{Error, ErrorId};
use hideout_contract::messages::UiAssetPut;
use hideout_contract::panel::MAX_ASSET_BYTES;
use std::collections::{HashMap, VecDeque};

/// Bytes all assets together may hold.
pub const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub mime: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct Assets {
    by_hash: HashMap<String, Asset>,
    order: VecDeque<String>,
    total: usize,
}

fn limits(message: impl Into<String>) -> Error {
    Error::new(ErrorId::UiLimits, message)
}

impl Assets {
    /// Stores one upload. The hash is the asset's name; the hub does not re-hash it, since a
    /// module that names its image wrongly only gets the wrong picture in its own panel.
    pub fn put(&mut self, put: &UiAssetPut) -> Result<(), Error> {
        let hash = put.hash.to_ascii_lowercase();
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(limits("asset hash is not 64 hex digits"));
        }
        if !put.mime.starts_with("image/") {
            return Err(limits(format!("asset type {} is not an image", put.mime)));
        }
        // Base64 grows by 4/3: refuse the text before decoding when it cannot fit.
        if put.data.len() > MAX_ASSET_BYTES.div_ceil(3) * 4 + 4 {
            return Err(limits("asset over 256 KiB"));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(put.data.trim())
            .map_err(|e| limits(format!("asset data: {e}")))?;
        if bytes.len() > MAX_ASSET_BYTES {
            return Err(limits("asset over 256 KiB"));
        }
        if let Some(old) = self.by_hash.remove(&hash) {
            self.total -= old.bytes.len();
            self.order.retain(|h| h != &hash);
        }
        self.total += bytes.len();
        self.by_hash.insert(
            hash.clone(),
            Asset {
                mime: put.mime.clone(),
                bytes,
            },
        );
        self.order.push_back(hash);
        while self.total > MAX_TOTAL_BYTES {
            let Some(h) = self.order.pop_front() else {
                break;
            };
            if let Some(a) = self.by_hash.remove(&h) {
                self.total -= a.bytes.len();
            }
        }
        Ok(())
    }

    pub fn get(&self, hash: &str) -> Option<&Asset> {
        self.by_hash.get(&hash.to_ascii_lowercase())
    }

    pub fn len(&self) -> usize {
        self.by_hash.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_hash.is_empty()
    }
}
