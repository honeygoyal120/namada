use borsh::{BorshDeserialize, BorshSerialize};
use namada_core::ledger::eth_bridge::storage::active_key;
use namada_core::ledger::eth_bridge::storage::bridge_pool::{
    get_nonce_key, get_signed_root_key,
};
use namada_core::ledger::storage;
use namada_core::ledger::storage::{Storage, StoreType};
use namada_core::types::address::Address;
use namada_core::types::ethereum_events::{EthAddress, Uint};
use namada_core::types::keccak::KeccakHash;
use namada_core::types::storage::{BlockHeight, Epoch};
use namada_core::types::token;
use namada_core::types::vote_extensions::validator_set_update::{
    EthAddrBook, ValidatorSetArgs, VotingPowersMap, VotingPowersMapExt,
};
use namada_core::types::voting_power::{
    EthBridgeVotingPower, FractionalVotingPower,
};
use namada_proof_of_stake::pos_queries::PosQueries;
use namada_proof_of_stake::PosBase;

use crate::storage::proof::EthereumProof;

/// This enum is used as a parameter to
/// [`EthBridgeQueries::must_send_valset_upd`].
pub enum SendValsetUpd {
    /// Check if it is possible to send a validator set update
    /// vote extension at the current block height.
    Now,
    /// Check if it is possible to send a validator set update
    /// vote extension at the previous block height.
    AtPrevHeight,
}

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize)]
/// An enum indicating if the Ethereum bridge is enabled.
pub enum EthBridgeStatus {
    Disabled,
    Enabled(EthBridgeEnabled),
}

#[derive(Debug, Clone, BorshDeserialize, BorshSerialize)]
/// Enum indicating if the bridge was initialized at genesis
/// or a later epoch.
pub enum EthBridgeEnabled {
    AtGenesis,
    AtEpoch(
        // bridge is enabled from this epoch
        // onwards. a validator set proof must
        // exist for this epoch.
        Epoch,
    ),
}

pub trait EthBridgeQueries {
    /// Check if the bridge is disabled, enabled, or
    /// scheduled to be enabled at a specified epoch.
    fn check_bridge_status(&self) -> EthBridgeStatus;

    /// Returns a boolean indicating whether the bridge
    /// is currently active.
    fn is_bridge_active(&self) -> bool;

    /// Fetch the first [`BlockHeight`] of the last [`Epoch`]
    /// committed to storage.
    fn get_epoch_start_height(&self) -> BlockHeight;

    /// Get the latest nonce for the Ethereum bridge
    /// pool.
    fn get_bridge_pool_nonce(&self) -> Uint;

    /// Get the nonce at a particular block height.
    fn get_bridge_pool_nonce_at_height(&self, height: BlockHeight) -> Uint;

    /// Get the latest root of the Ethereum bridge
    /// pool Merkle tree.
    fn get_bridge_pool_root(&self) -> KeccakHash;

    /// Get a quorum of validator signatures over
    /// the concatenation of the latest bridge pool
    /// root and nonce.
    ///
    /// No value exists when the bridge if first
    /// started.
    fn get_signed_bridge_pool_root(
        &self,
    ) -> Option<EthereumProof<(KeccakHash, Uint)>>;

    /// Get the root of the Ethereum bridge
    /// pool Merkle tree at a given height.
    fn get_bridge_pool_root_at_height(&self, height: BlockHeight)
    -> KeccakHash;

    /// Determines if it is possible to send a validator set update vote
    /// extension at the provided [`BlockHeight`] in [`SendValsetUpd`].
    fn must_send_valset_upd(&self, can_send: SendValsetUpd) -> bool;

    /// For a given Namada validator, return its corresponding Ethereum bridge
    /// address.
    fn get_ethbridge_from_namada_addr(
        &self,
        validator: &Address,
        epoch: Option<Epoch>,
    ) -> Option<EthAddress>;

    /// For a given Namada validator, return its corresponding Ethereum
    /// governance address.
    fn get_ethgov_from_namada_addr(
        &self,
        validator: &Address,
        epoch: Option<Epoch>,
    ) -> Option<EthAddress>;

    /// For a given Namada validator, return its corresponding Ethereum
    /// address book.
    #[inline]
    fn get_eth_addr_book(
        &self,
        validator: &Address,
        epoch: Option<Epoch>,
    ) -> Option<EthAddrBook> {
        let bridge = self.get_ethbridge_from_namada_addr(validator, epoch)?;
        let governance = self.get_ethgov_from_namada_addr(validator, epoch)?;
        Some(EthAddrBook {
            hot_key_addr: bridge,
            cold_key_addr: governance,
        })
    }

    /// Extension of [`Self::get_active_validators`], which additionally returns
    /// all Ethereum addresses of some validator.
    fn get_active_eth_addresses<'db>(
        &'db self,
        epoch: Option<Epoch>,
    ) -> Box<dyn Iterator<Item = (EthAddrBook, Address, token::Amount)> + 'db>;

    /// Query the active [`ValidatorSetArgs`] at the given [`Epoch`].
    fn get_validator_set_args(&self, epoch: Option<Epoch>) -> ValidatorSetArgs;
}

impl<D, H> EthBridgeQueries for Storage<D, H>
where
    D: storage::DB + for<'iter> storage::DBIter<'iter>,
    H: storage::StorageHasher,
{
    fn check_bridge_status(&self) -> EthBridgeStatus {
        BorshDeserialize::try_from_slice(
            self.read(&active_key())
                .expect(
                    "Reading the Ethereum bridge active key shouldn't fail.",
                )
                .0
                .expect("The Ethereum bridge active key should be in storage")
                .as_slice(),
        )
        .expect("Deserializing the Ethereum bridge active key shouldn't fail.")
    }

    fn is_bridge_active(&self) -> bool {
        if let EthBridgeStatus::Enabled(enabled_at) = self.check_bridge_status()
        {
            match enabled_at {
                EthBridgeEnabled::AtGenesis => true,
                EthBridgeEnabled::AtEpoch(epoch) => {
                    let current_epoch = self.get_current_epoch().0;
                    epoch <= current_epoch
                }
            }
        } else {
            false
        }
    }

    #[inline]
    fn get_epoch_start_height(&self) -> BlockHeight {
        // NOTE: the first stored height in `fst_block_heights_of_each_epoch`
        // is 0, because of a bug (should be 1), so this code needs to
        // handle that case
        //
        // we can remove this check once that's fixed
        if self.last_epoch.0 == 0 {
            return BlockHeight(1);
        }
        self.block
            .pred_epochs
            .first_block_heights()
            .last()
            .copied()
            .expect("The block height of the current epoch should be known")
    }

    fn get_bridge_pool_nonce(&self) -> Uint {
        Uint::try_from_slice(
            &self
                .read(&get_nonce_key())
                .expect("Reading Bridge pool nonce shouldn't fail.")
                .0
                .expect("Reading Bridge pool nonce shouldn't fail."),
        )
        .expect("Deserializing the nonce from storage should not fail.")
    }

    fn get_bridge_pool_nonce_at_height(&self, height: BlockHeight) -> Uint {
        Uint::try_from_slice(
            &self
                .db
                .read_subspace_val_with_height(
                    &get_nonce_key(),
                    height,
                    self.last_height,
                )
                .expect("Reading signed Bridge pool nonce shouldn't fail.")
                .expect("Reading signed Bridge pool nonce shouldn't fail."),
        )
        .expect("Deserializing the signed nonce from storage should not fail.")
    }

    fn get_bridge_pool_root(&self) -> KeccakHash {
        self.block.tree.sub_root(&StoreType::BridgePool).into()
    }

    fn get_signed_bridge_pool_root(
        &self,
    ) -> Option<EthereumProof<(KeccakHash, Uint)>> {
        self.read(&get_signed_root_key())
            .expect("Reading signed Bridge pool root shouldn't fail.")
            .0
            .map(|bytes| {
                BorshDeserialize::try_from_slice(&bytes).expect(
                    "Deserializing the signed bridge pool root from storage \
                     should not fail.",
                )
            })
    }

    fn get_bridge_pool_root_at_height(
        &self,
        height: BlockHeight,
    ) -> KeccakHash {
        self.db
            .read_merkle_tree_stores(height)
            .expect("We should always be able to read the database")
            .expect("Every root should correspond to an existing block height")
            .get_root(StoreType::BridgePool)
            .into()
    }

    #[cfg(feature = "abcipp")]
    #[inline]
    fn must_send_valset_upd(&self, can_send: SendValsetUpd) -> bool {
        if matches!(can_send, SendValsetUpd::Now) {
            self.is_deciding_offset_within_epoch(1)
        } else {
            // TODO: implement this method for ABCI++; should only be able to
            // send a validator set update at the second block of an
            // epoch
            false
        }
    }

    #[cfg(not(feature = "abcipp"))]
    #[inline]
    fn must_send_valset_upd(&self, can_send: SendValsetUpd) -> bool {
        if matches!(can_send, SendValsetUpd::AtPrevHeight) {
            // when checking vote extensions in Prepare
            // and ProcessProposal, we simply return true
            true
        } else {
            // offset of 1 => are we at the 2nd
            // block within the epoch?
            self.is_deciding_offset_within_epoch(1)
        }
    }

    #[inline]
    fn get_ethbridge_from_namada_addr(
        &self,
        validator: &Address,
        epoch: Option<Epoch>,
    ) -> Option<EthAddress> {
        let epoch = epoch.unwrap_or_else(|| self.get_current_epoch().0);
        self.read_validator_eth_hot_key(validator)
            .as_ref()
            .and_then(|epk| epk.get(epoch).and_then(|pk| pk.try_into().ok()))
    }

    #[inline]
    fn get_ethgov_from_namada_addr(
        &self,
        validator: &Address,
        epoch: Option<Epoch>,
    ) -> Option<EthAddress> {
        let epoch = epoch.unwrap_or_else(|| self.get_current_epoch().0);
        self.read_validator_eth_cold_key(validator)
            .as_ref()
            .and_then(|epk| epk.get(epoch).and_then(|pk| pk.try_into().ok()))
    }

    #[inline]
    fn get_active_eth_addresses<'db>(
        &'db self,
        epoch: Option<Epoch>,
    ) -> Box<dyn Iterator<Item = (EthAddrBook, Address, token::Amount)> + 'db>
    {
        let epoch = epoch.unwrap_or_else(|| self.get_current_epoch().0);
        Box::new(self.get_active_validators(Some(epoch)).into_iter().map(
            move |validator| {
                let hot_key_addr = self
                    .get_ethbridge_from_namada_addr(
                        &validator.address,
                        Some(epoch),
                    )
                    .expect(
                        "All Namada validators should have an Ethereum bridge \
                         key",
                    );
                let cold_key_addr = self
                    .get_ethgov_from_namada_addr(
                        &validator.address,
                        Some(epoch),
                    )
                    .expect(
                        "All Namada validators should have an Ethereum \
                         governance key",
                    );
                let eth_addr_book = EthAddrBook {
                    hot_key_addr,
                    cold_key_addr,
                };
                (
                    eth_addr_book,
                    validator.address,
                    validator.bonded_stake.into(),
                )
            },
        ))
    }

    fn get_validator_set_args(&self, epoch: Option<Epoch>) -> ValidatorSetArgs {
        let epoch = epoch.unwrap_or_else(|| self.get_current_epoch().0);

        let voting_powers_map: VotingPowersMap = self
            .get_active_eth_addresses(Some(epoch))
            .map(|(addr_book, _, power)| (addr_book, power))
            .collect();

        let total_power = self.get_total_voting_power(Some(epoch)).into();
        let (validators, voting_powers) = voting_powers_map
            .get_sorted()
            .into_iter()
            .map(|(&EthAddrBook { hot_key_addr, .. }, &power)| {
                let voting_power: EthBridgeVotingPower =
                    FractionalVotingPower::new(power.into(), total_power)
                        .expect("Fractional voting power should be >1")
                        .into();
                (hot_key_addr, voting_power)
            })
            .unzip();

        ValidatorSetArgs {
            epoch,
            validators,
            voting_powers,
        }
    }
}
