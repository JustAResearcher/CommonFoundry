use std::collections::{HashMap, HashSet};

use blake3::Hasher;
use cmfd_marketplace::{ChannelError, ChannelTerms, Settlement};
use k256::schnorr::{
    Signature, SigningKey, VerifyingKey,
    signature::{Signer, Verifier},
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    BlockChallenge, CoinbaseClaim, DifficultyError, EconomicsError, ForgeMatrixError,
    ForgeMatrixProof, ForgeMatrixVerifier, HeaderWork, MonetaryPolicy, next_work_target,
};

const TX_SIGNING_DOMAIN: &str = "CMFD/TRANSACTION/SIGNING/V1";
const TX_ID_DOMAIN: &str = "CMFD/TRANSACTION/ID/V1";
const COINBASE_ID_DOMAIN: &str = "CMFD/COINBASE/ID/V1";
const MERKLE_LEAF_DOMAIN: &str = "CMFD/MERKLE/LEAF/V1";
const MERKLE_NODE_DOMAIN: &str = "CMFD/MERKLE/NODE/V1";
const BLOCK_ID_DOMAIN: &str = "CMFD/BLOCK/ID/V1";

pub const COINBASE_MATURITY: u64 = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OutPoint {
    pub txid: [u8; 32],
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxOutput {
    pub value: u64,
    pub lock: OutputLock,
    pub spendable_height: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputLock {
    Key([u8; 32]),
    InferenceChannel { channel_id: [u8; 32] },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxInput {
    pub previous: OutPoint,
    pub witness: InputWitness,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputWitness {
    Key {
        public_key: [u8; 32],
        signature: Vec<u8>,
    },
    InferenceSettlement {
        terms: ChannelTerms,
        settlement: Settlement,
    },
    InferenceRefund {
        terms: ChannelTerms,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub version: u32,
    pub inputs: Vec<TxInput>,
    pub outputs: Vec<TxOutput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewardDestinations {
    pub miner: [u8; 32],
    pub steward: [u8; 32],
    pub community: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coinbase {
    pub height: u64,
    pub outputs: Vec<TxOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub challenge: BlockChallenge,
    pub proof: ForgeMatrixProof,
    pub coinbase: Coinbase,
    pub transactions: Vec<Transaction>,
}

#[derive(Debug, Clone, Default)]
pub struct UtxoSet {
    outputs: HashMap<OutPoint, TxOutput>,
    retired_channels: HashSet<[u8; 32]>,
}

#[derive(Debug, Clone)]
pub struct ChainState {
    utxos: UtxoSet,
    history: Vec<HeaderWork>,
    tip: [u8; 32],
    pow_limit: [u8; 32],
    next_height: u64,
    verifier: ForgeMatrixVerifier,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChainError {
    #[error("ForgeMatrix proof failed: {0}")]
    Proof(#[from] ForgeMatrixError),
    #[error("economics validation failed: {0}")]
    Economics(#[from] EconomicsError),
    #[error("inference channel validation failed: {0}")]
    Channel(#[from] ChannelError),
    #[error("difficulty calculation failed: {0}")]
    Difficulty(#[from] DifficultyError),
    #[error("unexpected block height: expected {expected}, received {actual}")]
    UnexpectedHeight { expected: u64, actual: u64 },
    #[error("previous block does not match the active chain tip")]
    PreviousBlock,
    #[error("block target does not equal the independently derived chain target")]
    UnexpectedTarget,
    #[error("block timestamp must increase relative to its parent")]
    TimestampNotIncreasing,
    #[error("block height and challenge height differ")]
    HeightMismatch,
    #[error("transaction Merkle root mismatch")]
    MerkleRoot,
    #[error("transaction has no inputs")]
    NoInputs,
    #[error("transaction has no outputs")]
    NoOutputs,
    #[error("input references a missing output")]
    MissingInput,
    #[error("an output is spent more than once in the block")]
    DuplicateInput,
    #[error("input is locked until height {0}")]
    ImmatureInput(u64),
    #[error("input public key does not own the referenced output")]
    WrongOwner,
    #[error("input witness cannot spend the referenced output lock")]
    WrongWitness,
    #[error("malformed Schnorr public key or signature")]
    MalformedSignature,
    #[error("Schnorr signature is invalid")]
    InvalidSignature,
    #[error("transaction outputs exceed transaction inputs")]
    CreatesValue,
    #[error("amount arithmetic overflow")]
    AmountOverflow,
    #[error("coinbase layout or destination is invalid")]
    CoinbaseLayout,
    #[error("transaction id collision")]
    TxidCollision,
    #[error("an inference channel identifier cannot be reused")]
    DuplicateChannel,
    #[error("inference channel funding value does not match its signed terms")]
    ChannelDeposit,
    #[error("an inference channel close must have one input and its exact consensus outputs")]
    ChannelCloseShape,
}

impl Transaction {
    pub fn signing_digest(&self) -> [u8; 32] {
        let mut hasher = Hasher::new_derive_key(TX_SIGNING_DOMAIN);
        encode_unsigned_transaction(self, &mut hasher);
        *hasher.finalize().as_bytes()
    }

    pub fn txid(&self) -> [u8; 32] {
        let mut hasher = Hasher::new_derive_key(TX_ID_DOMAIN);
        encode_unsigned_transaction(self, &mut hasher);
        for input in &self.inputs {
            encode_witness(&input.witness, true, &mut hasher);
        }
        *hasher.finalize().as_bytes()
    }

    pub fn sign_all(&mut self, keys: &[&SigningKey]) -> Result<(), ChainError> {
        if keys.len() != self.inputs.len() {
            return Err(ChainError::MalformedSignature);
        }
        for (input, key) in self.inputs.iter_mut().zip(keys) {
            let InputWitness::Key {
                public_key,
                signature,
            } = &mut input.witness
            else {
                return Err(ChainError::WrongWitness);
            };
            public_key.copy_from_slice(key.verifying_key().to_bytes().as_ref());
            signature.clear();
        }
        let digest = self.signing_digest();
        for (input, key) in self.inputs.iter_mut().zip(keys) {
            let signature: Signature = key.sign(&digest);
            let InputWitness::Key {
                signature: witness_signature,
                ..
            } = &mut input.witness
            else {
                return Err(ChainError::WrongWitness);
            };
            *witness_signature = signature.to_bytes().to_vec();
        }
        Ok(())
    }
}

impl Coinbase {
    pub fn new(
        height: u64,
        allocation: crate::Allocation,
        destinations: RewardDestinations,
    ) -> Self {
        let mut outputs = vec![TxOutput {
            value: allocation.miner,
            lock: OutputLock::Key(destinations.miner),
            spendable_height: height.saturating_add(COINBASE_MATURITY),
        }];
        if allocation.steward > 0 {
            outputs.push(TxOutput {
                value: allocation.steward,
                lock: OutputLock::Key(destinations.steward),
                spendable_height: height,
            });
        }
        if allocation.community > 0 {
            outputs.push(TxOutput {
                value: allocation.community,
                lock: OutputLock::Key(destinations.community),
                spendable_height: height,
            });
        }
        Self { height, outputs }
    }

    pub fn txid(&self) -> [u8; 32] {
        let mut hasher = Hasher::new_derive_key(COINBASE_ID_DOMAIN);
        hasher.update(&self.height.to_le_bytes());
        encode_outputs(&self.outputs, &mut hasher);
        *hasher.finalize().as_bytes()
    }
}

impl UtxoSet {
    pub fn get(&self, outpoint: &OutPoint) -> Option<&TxOutput> {
        self.outputs.get(outpoint)
    }

    #[cfg(test)]
    fn insert_for_testing(&mut self, outpoint: OutPoint, output: TxOutput) {
        self.outputs.insert(outpoint, output);
    }

    fn validate_and_apply(
        &mut self,
        block: &Block,
        verifier: &ForgeMatrixVerifier,
        policy: MonetaryPolicy,
        destinations: RewardDestinations,
    ) -> Result<u64, ChainError> {
        if block.challenge.height != block.coinbase.height {
            return Err(ChainError::HeightMismatch);
        }
        if block.transaction_root() != block.challenge.transaction_root {
            return Err(ChainError::MerkleRoot);
        }
        verifier.verify(&block.challenge, &block.proof)?;

        let mut candidate = self.clone();
        let mut spent = HashSet::new();
        let mut fees = 0u64;
        for transaction in &block.transactions {
            fees = fees
                .checked_add(candidate.apply_transaction(
                    transaction,
                    block.challenge.height,
                    &mut spent,
                )?)
                .ok_or(ChainError::AmountOverflow)?;
        }

        let allocation = policy.allocation(block.challenge.height, fees)?;
        validate_coinbase(&block.coinbase, allocation, destinations)?;
        let coinbase_txid = block.coinbase.txid();
        for (index, output) in block.coinbase.outputs.iter().enumerate() {
            let outpoint = OutPoint {
                txid: coinbase_txid,
                index: index as u32,
            };
            candidate.insert_output(outpoint, output.clone())?;
        }

        *self = candidate;
        Ok(fees)
    }

    fn apply_transaction(
        &mut self,
        transaction: &Transaction,
        height: u64,
        spent: &mut HashSet<OutPoint>,
    ) -> Result<u64, ChainError> {
        if transaction.inputs.is_empty() {
            return Err(ChainError::NoInputs);
        }
        if transaction.outputs.is_empty() {
            return Err(ChainError::NoOutputs);
        }

        if transaction
            .inputs
            .iter()
            .any(|input| !matches!(&input.witness, InputWitness::Key { .. }))
        {
            return self.apply_channel_transaction(transaction, height, spent);
        }

        let digest = transaction.signing_digest();
        let mut input_total = 0u64;
        for input in &transaction.inputs {
            if !spent.insert(input.previous) {
                return Err(ChainError::DuplicateInput);
            }
            let previous = self
                .outputs
                .get(&input.previous)
                .ok_or(ChainError::MissingInput)?;
            if height < previous.spendable_height {
                return Err(ChainError::ImmatureInput(previous.spendable_height));
            }
            let OutputLock::Key(owner) = previous.lock else {
                return Err(ChainError::WrongWitness);
            };
            let InputWitness::Key {
                public_key,
                signature,
            } = &input.witness
            else {
                return Err(ChainError::WrongWitness);
            };
            if *public_key != owner {
                return Err(ChainError::WrongOwner);
            }
            let key =
                VerifyingKey::from_bytes(public_key).map_err(|_| ChainError::MalformedSignature)?;
            let signature = Signature::try_from(signature.as_slice())
                .map_err(|_| ChainError::MalformedSignature)?;
            key.verify(&digest, &signature)
                .map_err(|_| ChainError::InvalidSignature)?;
            input_total = input_total
                .checked_add(previous.value)
                .ok_or(ChainError::AmountOverflow)?;
        }

        let output_total = transaction.outputs.iter().try_fold(0u64, |sum, output| {
            sum.checked_add(output.value)
                .ok_or(ChainError::AmountOverflow)
        })?;
        if output_total > input_total {
            return Err(ChainError::CreatesValue);
        }

        for input in &transaction.inputs {
            self.outputs.remove(&input.previous);
        }
        let txid = transaction.txid();
        for (index, output) in transaction.outputs.iter().enumerate() {
            let outpoint = OutPoint {
                txid,
                index: index as u32,
            };
            self.insert_output(outpoint, output.clone())?;
        }
        Ok(input_total - output_total)
    }

    fn apply_channel_transaction(
        &mut self,
        transaction: &Transaction,
        height: u64,
        spent: &mut HashSet<OutPoint>,
    ) -> Result<u64, ChainError> {
        if transaction.inputs.len() != 1 {
            return Err(ChainError::ChannelCloseShape);
        }
        let input = &transaction.inputs[0];
        if !spent.insert(input.previous) {
            return Err(ChainError::DuplicateInput);
        }
        let previous = self
            .outputs
            .get(&input.previous)
            .cloned()
            .ok_or(ChainError::MissingInput)?;
        if height < previous.spendable_height {
            return Err(ChainError::ImmatureInput(previous.spendable_height));
        }
        let OutputLock::InferenceChannel { channel_id } = previous.lock else {
            return Err(ChainError::WrongWitness);
        };

        let (expected_outputs, fee) = match &input.witness {
            InputWitness::InferenceSettlement { terms, settlement } => {
                if terms.channel_id()? != channel_id {
                    return Err(ChainError::WrongWitness);
                }
                if terms.deposit != previous.value {
                    return Err(ChainError::ChannelDeposit);
                }
                settlement.verify(terms)?;
                let state = &settlement.state.state;
                (
                    channel_outputs(
                        state.provider_payment,
                        terms.provider_key,
                        state.customer_refund,
                        terms.customer_key,
                        height,
                    ),
                    state.close_fee_burn,
                )
            }
            InputWitness::InferenceRefund { terms } => {
                if terms.channel_id()? != channel_id {
                    return Err(ChainError::WrongWitness);
                }
                if terms.deposit != previous.value {
                    return Err(ChainError::ChannelDeposit);
                }
                let refund = terms.refund(height)?;
                (
                    channel_outputs(
                        0,
                        terms.provider_key,
                        refund.customer_amount,
                        terms.customer_key,
                        height,
                    ),
                    refund.fee_burn,
                )
            }
            InputWitness::Key { .. } => return Err(ChainError::WrongWitness),
        };

        if transaction.outputs != expected_outputs {
            return Err(ChainError::ChannelCloseShape);
        }
        let output_total = transaction.outputs.iter().try_fold(0u64, |sum, output| {
            sum.checked_add(output.value)
                .ok_or(ChainError::AmountOverflow)
        })?;
        if output_total
            .checked_add(fee)
            .ok_or(ChainError::AmountOverflow)?
            != previous.value
        {
            return Err(ChainError::ChannelCloseShape);
        }

        self.outputs.remove(&input.previous);
        self.retired_channels.insert(channel_id);
        let txid = transaction.txid();
        for (index, output) in transaction.outputs.iter().enumerate() {
            self.insert_output(
                OutPoint {
                    txid,
                    index: index as u32,
                },
                output.clone(),
            )?;
        }
        Ok(fee)
    }

    fn insert_output(&mut self, outpoint: OutPoint, output: TxOutput) -> Result<(), ChainError> {
        if let OutputLock::InferenceChannel { channel_id } = &output.lock {
            let already_active = self.outputs.values().any(|existing| {
                matches!(
                    &existing.lock,
                    OutputLock::InferenceChannel {
                        channel_id: existing_id
                    } if existing_id == channel_id
                )
            });
            if already_active || self.retired_channels.contains(channel_id) {
                return Err(ChainError::DuplicateChannel);
            }
        }
        if self.outputs.insert(outpoint, output).is_some() {
            return Err(ChainError::TxidCollision);
        }
        Ok(())
    }
}

fn channel_outputs(
    provider_value: u64,
    provider_key: [u8; 32],
    customer_value: u64,
    customer_key: [u8; 32],
    height: u64,
) -> Vec<TxOutput> {
    let mut outputs = Vec::with_capacity(2);
    if provider_value > 0 {
        outputs.push(TxOutput {
            value: provider_value,
            lock: OutputLock::Key(provider_key),
            spendable_height: height,
        });
    }
    if customer_value > 0 {
        outputs.push(TxOutput {
            value: customer_value,
            lock: OutputLock::Key(customer_key),
            spendable_height: height,
        });
    }
    outputs
}

impl ChainState {
    pub fn new(
        genesis_hash: [u8; 32],
        pow_limit: [u8; 32],
        verifier: ForgeMatrixVerifier,
    ) -> Result<Self, ChainError> {
        // Validate the configured limit before accepting any chain state.
        next_work_target(&[], pow_limit)?;
        Ok(Self {
            utxos: UtxoSet::default(),
            history: Vec::new(),
            tip: genesis_hash,
            pow_limit,
            next_height: 1,
            verifier,
        })
    }

    pub fn utxos(&self) -> &UtxoSet {
        &self.utxos
    }

    pub fn tip(&self) -> [u8; 32] {
        self.tip
    }

    pub fn next_height(&self) -> u64 {
        self.next_height
    }

    pub fn expected_target(&self) -> Result<[u8; 32], ChainError> {
        Ok(next_work_target(&self.history, self.pow_limit)?)
    }

    pub fn validate_and_apply(
        &mut self,
        block: &Block,
        policy: MonetaryPolicy,
        destinations: RewardDestinations,
    ) -> Result<u64, ChainError> {
        if block.challenge.height != self.next_height {
            return Err(ChainError::UnexpectedHeight {
                expected: self.next_height,
                actual: block.challenge.height,
            });
        }
        if block.challenge.previous_block != self.tip {
            return Err(ChainError::PreviousBlock);
        }
        if self
            .history
            .last()
            .is_some_and(|header| block.challenge.timestamp <= header.timestamp)
        {
            return Err(ChainError::TimestampNotIncreasing);
        }
        if block.challenge.target != self.expected_target()? {
            return Err(ChainError::UnexpectedTarget);
        }

        let fees = self
            .utxos
            .validate_and_apply(block, &self.verifier, policy, destinations)?;
        self.history.push(HeaderWork {
            timestamp: block.challenge.timestamp,
            target: block.challenge.target,
        });
        self.tip = block.block_id();
        self.next_height = self
            .next_height
            .checked_add(1)
            .ok_or(ChainError::AmountOverflow)?;
        Ok(fees)
    }
}

impl Block {
    pub fn transaction_root(&self) -> [u8; 32] {
        let mut ids = Vec::with_capacity(self.transactions.len() + 1);
        ids.push(self.coinbase.txid());
        ids.extend(self.transactions.iter().map(Transaction::txid));
        merkle_root(&ids)
    }

    pub fn block_id(&self) -> [u8; 32] {
        let mut hasher = Hasher::new_derive_key(BLOCK_ID_DOMAIN);
        hasher.update(&self.challenge.previous_block);
        hasher.update(&self.challenge.transaction_root);
        hasher.update(&self.challenge.height.to_le_bytes());
        hasher.update(&self.challenge.timestamp.to_le_bytes());
        hasher.update(&self.challenge.target);
        hasher.update(&self.proof.algorithm_version.to_le_bytes());
        hasher.update(&self.proof.model_version.to_le_bytes());
        hasher.update(&self.proof.nonce.to_le_bytes());
        hasher.update(&self.proof.model_root);
        hasher.update(&self.proof.output_digest);
        hasher.update(&self.proof.work_digest);
        *hasher.finalize().as_bytes()
    }
}

fn validate_coinbase(
    coinbase: &Coinbase,
    allocation: crate::Allocation,
    destinations: RewardDestinations,
) -> Result<(), ChainError> {
    let expected = Coinbase::new(coinbase.height, allocation, destinations);
    if coinbase.outputs != expected.outputs {
        return Err(ChainError::CoinbaseLayout);
    }
    let claim = CoinbaseClaim {
        miner: coinbase.outputs.first().map_or(0, |output| output.value),
        steward: coinbase.outputs.get(1).map_or(0, |output| output.value),
        community: coinbase.outputs.get(2).map_or(0, |output| output.value),
    };
    if claim.miner != allocation.miner
        || claim.steward != allocation.steward
        || claim.community != allocation.community
    {
        return Err(ChainError::CoinbaseLayout);
    }
    Ok(())
}

pub fn merkle_root(txids: &[[u8; 32]]) -> [u8; 32] {
    if txids.is_empty() {
        return *Hasher::new_derive_key(MERKLE_LEAF_DOMAIN)
            .finalize()
            .as_bytes();
    }
    let mut level: Vec<[u8; 32]> = txids
        .iter()
        .map(|txid| {
            let mut hasher = Hasher::new_derive_key(MERKLE_LEAF_DOMAIN);
            hasher.update(txid);
            *hasher.finalize().as_bytes()
        })
        .collect();
    while level.len() > 1 {
        if level.len() % 2 == 1 {
            level.push(*level.last().expect("nonempty Merkle level"));
        }
        level = level
            .chunks_exact(2)
            .map(|pair| {
                let mut hasher = Hasher::new_derive_key(MERKLE_NODE_DOMAIN);
                hasher.update(&pair[0]);
                hasher.update(&pair[1]);
                *hasher.finalize().as_bytes()
            })
            .collect();
    }
    level[0]
}

fn encode_unsigned_transaction(transaction: &Transaction, hasher: &mut Hasher) {
    hasher.update(&transaction.version.to_le_bytes());
    hasher.update(&(transaction.inputs.len() as u64).to_le_bytes());
    for input in &transaction.inputs {
        hasher.update(&input.previous.txid);
        hasher.update(&input.previous.index.to_le_bytes());
        encode_witness(&input.witness, false, hasher);
    }
    encode_outputs(&transaction.outputs, hasher);
}

fn encode_outputs(outputs: &[TxOutput], hasher: &mut Hasher) {
    hasher.update(&(outputs.len() as u64).to_le_bytes());
    for output in outputs {
        hasher.update(&output.value.to_le_bytes());
        match &output.lock {
            OutputLock::Key(owner) => {
                hasher.update(&[0]);
                hasher.update(owner);
            }
            OutputLock::InferenceChannel { channel_id } => {
                hasher.update(&[1]);
                hasher.update(channel_id);
            }
        }
        hasher.update(&output.spendable_height.to_le_bytes());
    }
}

fn encode_witness(witness: &InputWitness, include_signatures: bool, hasher: &mut Hasher) {
    match witness {
        InputWitness::Key {
            public_key,
            signature,
        } => {
            hasher.update(&[0]);
            hasher.update(public_key);
            if include_signatures {
                encode_bytes(signature, hasher);
            }
        }
        InputWitness::InferenceSettlement { terms, settlement } => {
            hasher.update(&[1]);
            encode_channel_terms(terms, hasher);
            encode_payment_state(&settlement.state.state, hasher);
            if include_signatures {
                encode_bytes(&settlement.state.customer_signature, hasher);
                encode_bytes(&settlement.provider_signature, hasher);
                hasher.update(&settlement.witness_digest());
            }
        }
        InputWitness::InferenceRefund { terms } => {
            hasher.update(&[2]);
            encode_channel_terms(terms, hasher);
        }
    }
}

fn encode_channel_terms(terms: &ChannelTerms, hasher: &mut Hasher) {
    hasher.update(&terms.network_id);
    hasher.update(&terms.job_id);
    hasher.update(&terms.customer_key);
    hasher.update(&terms.provider_key);
    hasher.update(&terms.model_digest);
    hasher.update(&terms.runtime_digest);
    hasher.update(&terms.input_digest);
    for value in [
        terms.deposit,
        terms.close_fee_burn,
        terms.base_price,
        terms.input_price_per_1000,
        terms.output_price_per_1000,
        terms.max_input_tokens,
        terms.max_output_tokens,
        terms.output_chunk_tokens,
        terms.refund_height,
    ] {
        hasher.update(&value.to_le_bytes());
    }
}

fn encode_payment_state(state: &cmfd_marketplace::PaymentState, hasher: &mut Hasher) {
    hasher.update(&state.channel_id);
    for value in [
        state.sequence,
        state.input_tokens,
        state.authorized_output_tokens,
        state.provider_payment,
        state.customer_refund,
        state.close_fee_burn,
    ] {
        hasher.update(&value.to_le_bytes());
    }
    hasher.update(&state.previous_receipt);
}

fn encode_bytes(bytes: &[u8], hasher: &mut Hasher) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DEFAULT_MONETARY_POLICY, ForgeMatrixProfile, TEST_PROFILE};
    use cmfd_marketplace::{ChannelTerms, Settlement, SignedPaymentState};

    fn signing_key(byte: u8) -> SigningKey {
        SigningKey::from_bytes(&[byte; 32]).unwrap()
    }

    fn owner(key: &SigningKey) -> [u8; 32] {
        key.verifying_key().to_bytes().into()
    }

    fn destinations() -> RewardDestinations {
        RewardDestinations {
            miner: owner(&signing_key(2)),
            steward: owner(&signing_key(3)),
            community: owner(&signing_key(4)),
        }
    }

    fn channel_terms(customer: &SigningKey, provider: &SigningKey) -> ChannelTerms {
        ChannelTerms {
            network_id: [1; 32],
            job_id: [2; 32],
            customer_key: owner(customer),
            provider_key: owner(provider),
            model_digest: [3; 32],
            runtime_digest: [4; 32],
            input_digest: [5; 32],
            deposit: 1_000_000,
            close_fee_burn: 1_000,
            base_price: 100,
            input_price_per_1000: 2_000,
            output_price_per_1000: 4_000,
            max_input_tokens: 8_192,
            max_output_tokens: 1_024,
            output_chunk_tokens: 32,
            refund_height: 500,
        }
    }

    fn block_with_transactions(
        height: u64,
        previous_block: [u8; 32],
        transactions: Vec<Transaction>,
        fees: u64,
        verifier: &ForgeMatrixVerifier,
    ) -> Block {
        let allocation = DEFAULT_MONETARY_POLICY.allocation(height, fees).unwrap();
        let coinbase = Coinbase::new(height, allocation, destinations());
        let mut ids = vec![coinbase.txid()];
        ids.extend(transactions.iter().map(Transaction::txid));
        let challenge = BlockChallenge {
            previous_block,
            transaction_root: merkle_root(&ids),
            height,
            timestamp: height * 60,
            target: [0xff; 32],
        };
        Block {
            proof: verifier.prove(&challenge, 0),
            challenge,
            coinbase,
            transactions,
        }
    }

    #[test]
    fn signed_utxo_spend_burns_fee_and_block_applies_atomically() {
        let spend_key = signing_key(1);
        let previous = OutPoint {
            txid: [9; 32],
            index: 0,
        };
        let mut utxos = UtxoSet::default();
        utxos.insert_for_testing(
            previous,
            TxOutput {
                value: 10_000,
                lock: OutputLock::Key(owner(&spend_key)),
                spendable_height: 0,
            },
        );

        let mut transaction = Transaction {
            version: 1,
            inputs: vec![TxInput {
                previous,
                witness: InputWitness::Key {
                    public_key: [0; 32],
                    signature: vec![],
                },
            }],
            outputs: vec![TxOutput {
                value: 9_000,
                lock: OutputLock::Key(owner(&signing_key(5))),
                spendable_height: 0,
            }],
        };
        transaction.sign_all(&[&spend_key]).unwrap();

        let verifier = ForgeMatrixVerifier::new(TEST_PROFILE).unwrap();
        let allocation = DEFAULT_MONETARY_POLICY.allocation(1, 1_000).unwrap();
        let coinbase = Coinbase::new(1, allocation, destinations());
        let tx_root = merkle_root(&[coinbase.txid(), transaction.txid()]);
        let challenge = BlockChallenge {
            previous_block: [7; 32],
            transaction_root: tx_root,
            height: 1,
            timestamp: 1,
            target: [0xff; 32],
        };
        let proof = verifier.prove(&challenge, 0);
        let block = Block {
            challenge,
            proof,
            coinbase,
            transactions: vec![transaction],
        };

        assert_eq!(
            utxos
                .validate_and_apply(&block, &verifier, DEFAULT_MONETARY_POLICY, destinations())
                .unwrap(),
            1_000
        );
        assert!(utxos.get(&previous).is_none());
    }

    #[test]
    fn coinbase_cannot_claim_burned_fee() {
        let verifier = ForgeMatrixVerifier::new(TEST_PROFILE).unwrap();
        let allocation = DEFAULT_MONETARY_POLICY.allocation(1, 1_000).unwrap();
        let mut coinbase = Coinbase::new(1, allocation, destinations());
        coinbase.outputs[0].value += 1_000;
        let root = merkle_root(&[coinbase.txid()]);
        let challenge = BlockChallenge {
            previous_block: [7; 32],
            transaction_root: root,
            height: 1,
            timestamp: 1,
            target: [0xff; 32],
        };
        let block = Block {
            proof: verifier.prove(&challenge, 0),
            challenge,
            coinbase,
            transactions: vec![],
        };
        assert_eq!(
            UtxoSet::default().validate_and_apply(
                &block,
                &verifier,
                DEFAULT_MONETARY_POLICY,
                destinations()
            ),
            Err(ChainError::CoinbaseLayout)
        );
    }

    #[test]
    fn steward_and_community_are_paid_and_spendable_per_block() {
        let reward_destinations = destinations();
        for height in [1, 2, DEFAULT_MONETARY_POLICY.halving_interval] {
            let allocation = DEFAULT_MONETARY_POLICY.allocation(height, 77).unwrap();
            let coinbase = Coinbase::new(height, allocation, reward_destinations);
            assert_eq!(coinbase.outputs.len(), 3);
            assert_eq!(coinbase.outputs[0].value, allocation.miner);
            assert_eq!(
                coinbase.outputs[0].spendable_height,
                height + COINBASE_MATURITY
            );
            assert_eq!(coinbase.outputs[1].value, allocation.steward);
            assert_eq!(
                coinbase.outputs[1].lock,
                OutputLock::Key(reward_destinations.steward)
            );
            assert_eq!(coinbase.outputs[1].spendable_height, height);
            assert_eq!(coinbase.outputs[2].value, allocation.community);
            assert_eq!(
                coinbase.outputs[2].lock,
                OutputLock::Key(reward_destinations.community)
            );
            assert_eq!(coinbase.outputs[2].spendable_height, height);
        }
    }

    #[test]
    fn chain_derives_target_and_parent_instead_of_trusting_miner() {
        let verifier = ForgeMatrixVerifier::new(TEST_PROFILE).unwrap();
        let genesis = [7; 32];
        let pow_limit = [0xff; 32];
        let mut state = ChainState::new(genesis, pow_limit, verifier.clone()).unwrap();
        let allocation = DEFAULT_MONETARY_POLICY.allocation(1, 0).unwrap();
        let coinbase = Coinbase::new(1, allocation, destinations());
        let root = merkle_root(&[coinbase.txid()]);
        let challenge = BlockChallenge {
            previous_block: genesis,
            transaction_root: root,
            height: 1,
            timestamp: 60,
            target: pow_limit,
        };
        let block = Block {
            proof: verifier.prove(&challenge, 0),
            challenge,
            coinbase,
            transactions: vec![],
        };

        let mut easy_target = block.clone();
        easy_target.challenge.target[0] = 0xfe;
        easy_target.proof = verifier.prove(&easy_target.challenge, 0);
        assert_eq!(
            state.validate_and_apply(&easy_target, DEFAULT_MONETARY_POLICY, destinations()),
            Err(ChainError::UnexpectedTarget)
        );

        state
            .validate_and_apply(&block, DEFAULT_MONETARY_POLICY, destinations())
            .unwrap();
        assert_eq!(state.tip(), block.block_id());

        let mut wrong_parent = block;
        wrong_parent.challenge.height = 2;
        wrong_parent.coinbase.height = 2;
        wrong_parent.challenge.previous_block = [8; 32];
        wrong_parent.challenge.transaction_root = merkle_root(&[wrong_parent.coinbase.txid()]);
        wrong_parent.proof = verifier.prove(&wrong_parent.challenge, 0);
        assert_eq!(
            state.validate_and_apply(&wrong_parent, DEFAULT_MONETARY_POLICY, destinations()),
            Err(ChainError::PreviousBlock)
        );
    }

    #[test]
    fn chain_owned_verifier_rejects_a_different_profile() {
        let _chain_owned_verifier_api: fn(
            &mut ChainState,
            &Block,
            MonetaryPolicy,
            RewardDestinations,
        ) -> Result<u64, ChainError> = ChainState::validate_and_apply;

        let genesis = [7; 32];
        let pow_limit = [0xff; 32];
        let consensus_verifier = ForgeMatrixVerifier::new(TEST_PROFILE).unwrap();
        let shortcut_profile = ForgeMatrixProfile {
            layers: 1,
            ..TEST_PROFILE
        };
        let shortcut_verifier = ForgeMatrixVerifier::new(shortcut_profile).unwrap();
        let mut state = ChainState::new(genesis, pow_limit, consensus_verifier).unwrap();

        let allocation = DEFAULT_MONETARY_POLICY.allocation(1, 0).unwrap();
        let coinbase = Coinbase::new(1, allocation, destinations());
        let challenge = BlockChallenge {
            previous_block: genesis,
            transaction_root: merkle_root(&[coinbase.txid()]),
            height: 1,
            timestamp: 60,
            target: pow_limit,
        };
        let block = Block {
            proof: shortcut_verifier.prove(&challenge, 0),
            challenge,
            coinbase,
            transactions: vec![],
        };

        assert_eq!(
            state.validate_and_apply(&block, DEFAULT_MONETARY_POLICY, destinations()),
            Err(ChainError::Proof(ForgeMatrixError::ModelRoot))
        );
        assert_eq!(state.tip(), genesis);
        assert_eq!(state.next_height(), 1);
    }

    #[test]
    fn inference_channel_settlement_pays_gpu_and_burns_close_fee() {
        let customer = signing_key(10);
        let provider = signing_key(11);
        let terms = channel_terms(&customer, &provider);
        let channel_id = terms.channel_id().unwrap();
        let funding = OutPoint {
            txid: [12; 32],
            index: 0,
        };
        let mut utxos = UtxoSet::default();
        utxos.insert_for_testing(
            funding,
            TxOutput {
                value: terms.deposit,
                lock: OutputLock::InferenceChannel { channel_id },
                spendable_height: 0,
            },
        );

        let state = SignedPaymentState::sign(terms.initial_state(100, 32).unwrap(), &customer);
        let settlement = Settlement::close(state, &provider);
        let outputs = channel_outputs(
            settlement.state.state.provider_payment,
            terms.provider_key,
            settlement.state.state.customer_refund,
            terms.customer_key,
            1,
        );
        let transaction = Transaction {
            version: 1,
            inputs: vec![TxInput {
                previous: funding,
                witness: InputWitness::InferenceSettlement {
                    terms: terms.clone(),
                    settlement,
                },
            }],
            outputs,
        };
        let transaction_id = transaction.txid();
        let verifier = ForgeMatrixVerifier::new(TEST_PROFILE).unwrap();
        let block = block_with_transactions(
            1,
            [7; 32],
            vec![transaction],
            terms.close_fee_burn,
            &verifier,
        );

        let mut redirected = block.clone();
        redirected.transactions[0].outputs[0].lock = OutputLock::Key(terms.customer_key);
        redirected.challenge.transaction_root = redirected.transaction_root();
        redirected.proof = verifier.prove(&redirected.challenge, 0);
        assert_eq!(
            utxos.validate_and_apply(
                &redirected,
                &verifier,
                DEFAULT_MONETARY_POLICY,
                destinations()
            ),
            Err(ChainError::ChannelCloseShape)
        );

        assert_eq!(
            utxos
                .validate_and_apply(&block, &verifier, DEFAULT_MONETARY_POLICY, destinations())
                .unwrap(),
            1_000
        );
        assert!(utxos.get(&funding).is_none());
        assert_eq!(
            utxos
                .get(&OutPoint {
                    txid: transaction_id,
                    index: 0
                })
                .unwrap()
                .value,
            428
        );
        assert_eq!(
            utxos
                .get(&OutPoint {
                    txid: transaction_id,
                    index: 1
                })
                .unwrap()
                .value,
            998_572
        );
    }

    #[test]
    fn inference_channel_refund_is_timelocked_and_exact() {
        let customer = signing_key(10);
        let provider = signing_key(11);
        let terms = channel_terms(&customer, &provider);
        let funding = OutPoint {
            txid: [13; 32],
            index: 0,
        };
        let mut utxos = UtxoSet::default();
        utxos.insert_for_testing(
            funding,
            TxOutput {
                value: terms.deposit,
                lock: OutputLock::InferenceChannel {
                    channel_id: terms.channel_id().unwrap(),
                },
                spendable_height: 0,
            },
        );
        let refund_transaction = |height| Transaction {
            version: 1,
            inputs: vec![TxInput {
                previous: funding,
                witness: InputWitness::InferenceRefund {
                    terms: terms.clone(),
                },
            }],
            outputs: channel_outputs(
                0,
                terms.provider_key,
                terms.deposit - terms.close_fee_burn,
                terms.customer_key,
                height,
            ),
        };
        let verifier = ForgeMatrixVerifier::new(TEST_PROFILE).unwrap();
        let early = block_with_transactions(
            499,
            [7; 32],
            vec![refund_transaction(499)],
            terms.close_fee_burn,
            &verifier,
        );
        assert_eq!(
            utxos.validate_and_apply(&early, &verifier, DEFAULT_MONETARY_POLICY, destinations()),
            Err(ChainError::Channel(ChannelError::RefundLocked))
        );

        let mature = block_with_transactions(
            500,
            [7; 32],
            vec![refund_transaction(500)],
            terms.close_fee_burn,
            &verifier,
        );
        assert_eq!(
            utxos
                .validate_and_apply(&mature, &verifier, DEFAULT_MONETARY_POLICY, destinations())
                .unwrap(),
            1_000
        );
        assert!(utxos.get(&funding).is_none());
    }
}
