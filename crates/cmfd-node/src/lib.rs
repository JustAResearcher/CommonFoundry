use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use blake3::Hasher;
use cmfd_consensus::chain::ValidatedBlock;
use cmfd_consensus::forgematrix::target_with_leading_zero_bits;
use cmfd_consensus::{
    BLOCK_VERSION, Block, BlockChallenge, BlockProof, BlockValidationContext, COIN, ChainError,
    ChainState, Coinbase, ConsensusPowVerifier, DEFAULT_MONETARY_POLICY, EconomicsError,
    FixedRewardDestinations, InputWitness, MAX_BLOCK_BYTES, MAX_FUTURE_OFFSET_SECS,
    MAX_TRANSACTION_BYTES, MAX_TRANSACTION_INPUTS, NETWORK_PROTOCOL_VERSION, NetworkError,
    NetworkParams, OutPoint, OutputLock, PowError, PowParameters, TRANSACTION_VERSION, Transaction,
    TxInput, TxOutput, WireError, add_chain_work, chain_work_bytes, decode_block,
    decode_transaction, encode_block, encode_transaction, merkle_root, v2_test_reference,
};
use fs2::FileExt;
use k256::schnorr::{SigningKey, VerifyingKey};
use primitive_types::U512;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;

pub mod p2p;
pub mod peer;

pub const DEVNET_NETWORK_ID: [u8; 32] = [0x63; 32];
pub const DEVNET_GENESIS_HASH: [u8; 32] = [0x47; 32];
pub const DEVNET_GENESIS_TIMESTAMP: u64 = 1_700_000_000;
pub const DEFAULT_RPC_ADDRESS: &str = "127.0.0.1:18443";
pub const DEFAULT_P2P_ADDRESS: &str = "127.0.0.1:18444";
pub const DEFAULT_DATA_DIR: &str = "commonfoundry-devnet0";
pub const DEFAULT_MINING_ATTEMPTS: u64 = 1_000_000;
pub const MAX_MEMPOOL_TRANSACTIONS: usize = 1_024;
pub const MAX_MEMPOOL_BYTES: usize = 512 * 1024;
pub const MIN_RELAY_FEE_PER_KIB: u64 = 1;
pub const MAX_WALLET_HISTORY: usize = 100;

const METADATA_FILE: &str = "network.meta";
const BLOCK_LOG_FILE: &str = "blocks.log";
const LOCK_FILE: &str = "node.lock";
const METADATA_MAGIC: [u8; 4] = *b"CMFM";
const METADATA_VERSION: u16 = 1;
const METADATA_BYTES: usize = 40;
const RECORD_MAGIC: [u8; 4] = *b"CMFR";
const RECORD_VERSION: u16 = 1;
const RECORD_HEADER_BYTES: usize = 20;
const RECORD_CHECKSUM_BYTES: usize = 32;
const RECORD_CHECKSUM_DOMAIN: &str = "CMFD/NODE/BLOCK-RECORD/V1";
const RPC_HEADER_LIMIT: usize = 8 * 1024;
const RPC_READ_TIMEOUT: Duration = Duration::from_secs(5);
const RPC_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const RPC_TOTAL_READ_TIMEOUT: Duration = Duration::from_secs(10);
const WALLET_JSON_BODY_LIMIT: usize = 2 * 1024;
const DEV_WALLET_WARNING: &str = "INSECURE DEVNET-0 DEMO WALLET: the private key is public in the source code; these coins are valueless and this wallet must never be used on a production network.";

#[derive(Debug, Error)]
pub enum NodeError {
    #[error("data directory {0:?} is locked by another running node")]
    DataDirLocked(PathBuf),
    #[error("{operation} failed for {path:?}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("network metadata is corrupt or unsupported")]
    InvalidMetadata,
    #[error("network metadata is missing while a nonempty block log already exists")]
    MissingMetadata,
    #[error("data directory belongs to a different immutable network fingerprint")]
    FingerprintMismatch,
    #[error("block log is corrupt: {0}")]
    CorruptLog(String),
    #[error("system clock is before the Unix epoch")]
    InvalidSystemTime,
    #[error("no currently valid template timestamp exists; wait for wall clock time to catch up")]
    TemplateTimeUnavailable,
    #[error("miner destination must be a 64-character hex Schnorr public key")]
    InvalidMinerDestination,
    #[error("RPC bind address must be loopback, received {0}")]
    NonLoopbackRpc(SocketAddr),
    #[error(
        "block storage is faulted after an append or sync failure; restart only after inspecting the log"
    )]
    StorageFaulted,
    #[error("block is already indexed: {0:?}")]
    DuplicateBlock([u8; 32]),
    #[error("block parent is not indexed: {0:?}")]
    UnknownParent([u8; 32]),
    #[error("transaction is already in the mempool: {0:?}")]
    DuplicateMempoolTransaction([u8; 32]),
    #[error("transaction input conflicts with the first mempool spend: {0:?}")]
    MempoolInputConflict(OutPoint),
    #[error("transaction input is not confirmed on the active chain: {0:?}")]
    MempoolUnconfirmedInput(OutPoint),
    #[error("mempool transaction count would exceed {MAX_MEMPOOL_TRANSACTIONS}")]
    MempoolTransactionLimit,
    #[error("mempool bytes would exceed {MAX_MEMPOOL_BYTES}")]
    MempoolByteLimit,
    #[error("transaction burns a fee of {actual}, below the relay minimum of {required}")]
    MempoolFeeTooLow { required: u64, actual: u64 },
    #[error("wallet recipient must be a 64-character hex Schnorr public key")]
    InvalidWalletRecipient,
    #[error("wallet amount must be an unsigned decimal CMFD value with at most 8 decimal places")]
    InvalidWalletAmount,
    #[error("wallet amount arithmetic overflow")]
    WalletAmountOverflow,
    #[error("wallet send amount must be nonzero")]
    WalletZeroAmount,
    #[error("wallet funds are immature: {immature} atoms immature, {required} atoms required")]
    WalletFundsImmature { immature: u64, required: u64 },
    #[error(
        "wallet has insufficient funds: {available} atoms available, {required} atoms required"
    )]
    WalletInsufficientFunds { available: u64, required: u64 },
    #[error(
        "wallet send would require more than {MAX_TRANSACTION_INPUTS} inputs: {selected} atoms selected, {required} atoms required"
    )]
    WalletInputLimit { selected: u64, required: u64 },
    #[error("wallet consolidation max_inputs must be between 2 and {MAX_TRANSACTION_INPUTS}")]
    InvalidConsolidationMaxInputs,
    #[error("wallet consolidation requires at least two mature unreserved outputs, found {0}")]
    WalletNotEnoughUtxos(usize),
    #[error("wallet consolidation fee must be less than its {input} atom input total")]
    WalletConsolidationFee { input: u64 },
    #[error("RPC request is invalid: {0}")]
    InvalidRpcRequest(String),
    #[error("RPC I/O failed: {0}")]
    RpcIo(#[source] io::Error),
    #[error("shared node mutex is poisoned")]
    SharedNodePoisoned,
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Network(#[from] NetworkError),
    #[error(transparent)]
    Chain(#[from] ChainError),
    #[error(transparent)]
    Wire(#[from] WireError),
    #[error(transparent)]
    Pow(#[from] PowError),
    #[error(transparent)]
    Economics(#[from] EconomicsError),
}

#[derive(Debug, Clone, Serialize)]
pub struct NodeStatus {
    pub network: &'static str,
    pub network_id: String,
    pub consensus_fingerprint: String,
    pub proof_of_work: &'static str,
    pub tip: String,
    pub cumulative_work: String,
    pub accepted_height: u64,
    pub next_height: u64,
    pub expected_target: String,
    pub utxo_count: usize,
    pub mempool_transactions: usize,
    pub mempool_bytes: usize,
    pub storage_healthy: bool,
}

#[derive(Debug, Clone)]
pub struct BlockTemplate {
    pub challenge: BlockChallenge,
    pub coinbase: Coinbase,
    pub transactions: Vec<Transaction>,
    pub total_fees_burned: u64,
}

#[derive(Debug, Clone)]
pub struct MempoolEntry {
    pub txid: [u8; 32],
    pub transaction: Transaction,
    pub encoded_bytes: usize,
    pub fee_burned: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WalletBalances {
    pub spendable_atoms: String,
    pub immature_atoms: String,
    pub pending_atoms: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WalletMempoolStatus {
    pub transactions: usize,
    pub bytes: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WalletHistoryEntry {
    pub kind: &'static str,
    pub txid: String,
    pub height: Option<u64>,
    pub timestamp: Option<u64>,
    pub confirmations: u64,
    pub status: &'static str,
    pub net_amount_atoms: String,
    pub fee_burned_atoms: String,
    pub counterparty: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WalletSnapshot {
    pub network: &'static str,
    pub devnet_only: bool,
    pub insecure_demo_wallet: bool,
    pub warning: &'static str,
    pub destination: String,
    pub accepted_height: u64,
    pub next_height: u64,
    pub balances: WalletBalances,
    pub spendable_utxo_count: usize,
    pub immature_utxo_count: usize,
    pub reserved_utxo_count: usize,
    pub mempool: WalletMempoolStatus,
    pub history_limit: usize,
    pub history: Vec<WalletHistoryEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletSendRequest {
    recipient: String,
    amount: String,
    fee: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletConsolidateRequest {
    fee: String,
    max_inputs: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WalletSendResponse {
    pub network: &'static str,
    pub devnet_only: bool,
    pub insecure_demo_wallet: bool,
    pub warning: &'static str,
    pub txid: String,
    pub amount_atoms: String,
    pub fee_burned_atoms: String,
    pub change_atoms: String,
    pub mempool_transactions: usize,
    pub mempool_bytes: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WalletConsolidateResponse {
    pub network: &'static str,
    pub devnet_only: bool,
    pub insecure_demo_wallet: bool,
    pub warning: &'static str,
    pub txid: String,
    pub inputs_consolidated: usize,
    pub input_atoms: String,
    pub output_atoms: String,
    pub fee_burned_atoms: String,
    pub mempool_transactions: usize,
    pub mempool_bytes: usize,
}

struct DataDirLock {
    _file: File,
}

impl DataDirLock {
    fn acquire(data_dir: &Path) -> Result<Self, NodeError> {
        fs::create_dir_all(data_dir)
            .map_err(|source| io_error("create data directory", data_dir, source))?;
        let path = data_dir.join(LOCK_FILE);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| io_error("open data-directory lock file", &path, source))?;
        if let Err(source) = FileExt::try_lock_exclusive(&file) {
            let expected = fs2::lock_contended_error();
            let contended = match (source.raw_os_error(), expected.raw_os_error()) {
                (Some(actual), Some(expected)) => actual == expected,
                _ => source.kind() == expected.kind(),
            };
            if contended {
                return Err(NodeError::DataDirLocked(path));
            }
            return Err(io_error("lock data directory", &path, source));
        }
        file.set_len(0)
            .map_err(|source| io_error("refresh lock file", &path, source))?;
        writeln!(&mut file, "pid={}", std::process::id())
            .map_err(|source| io_error("write lock file", &path, source))?;
        file.sync_all()
            .map_err(|source| io_error("sync lock file", &path, source))?;
        Ok(Self { _file: file })
    }
}

pub struct Node {
    data_dir: PathBuf,
    params: NetworkParams,
    fingerprint: [u8; 32],
    verifier: ConsensusPowVerifier,
    state: ChainState,
    index: BlockIndex,
    mempool: BTreeMap<[u8; 32], MempoolEntry>,
    mempool_bytes: usize,
    log: File,
    storage_faulted: bool,
    _lock: DataDirLock,
}

/// A fully validated block retained by the private Devnet-0 fork index.
///
/// Devnet-0 deliberately favors a small, auditable implementation over scale:
/// side branches are reconstructed from genesis when they are extended.
#[derive(Debug, Clone)]
struct IndexedBlock {
    parent: [u8; 32],
    height: u64,
    accepted_at: u64,
    canonical: Vec<u8>,
    cumulative_work: U512,
}

#[derive(Debug)]
struct BlockIndex {
    genesis: [u8; 32],
    blocks: HashMap<[u8; 32], IndexedBlock>,
    /// Active block identifiers in height order, including virtual genesis at
    /// index zero.
    active_chain: Vec<[u8; 32]>,
    active_work: U512,
}

impl BlockIndex {
    fn new(genesis: [u8; 32]) -> Self {
        Self {
            genesis,
            blocks: HashMap::new(),
            active_chain: vec![genesis],
            active_work: U512::zero(),
        }
    }

    fn contains(&self, block_id: [u8; 32]) -> bool {
        block_id == self.genesis || self.blocks.contains_key(&block_id)
    }

    fn work_at(&self, block_id: [u8; 32]) -> Option<U512> {
        if block_id == self.genesis {
            Some(U512::zero())
        } else {
            self.blocks
                .get(&block_id)
                .map(|entry| entry.cumulative_work)
        }
    }

    fn path_to(&self, tip: [u8; 32]) -> Result<Vec<[u8; 32]>, NodeError> {
        if tip == self.genesis {
            return Ok(Vec::new());
        }
        let mut reversed = Vec::new();
        let mut cursor = tip;
        while cursor != self.genesis {
            if reversed.len() >= self.blocks.len() {
                return Err(NodeError::CorruptLog(
                    "fork index contains a parent cycle".to_owned(),
                ));
            }
            let entry = self.blocks.get(&cursor).ok_or_else(|| {
                NodeError::CorruptLog("fork index contains a missing parent".to_owned())
            })?;
            reversed.push(cursor);
            cursor = entry.parent;
        }
        reversed.reverse();
        Ok(reversed)
    }

    fn active_position(&self, block_id: [u8; 32]) -> Option<usize> {
        self.active_chain
            .iter()
            .position(|candidate| *candidate == block_id)
    }
}

enum ValidatedCandidate {
    Active(ValidatedBlock),
    Branch {
        state: Box<ChainState>,
        validated: ValidatedBlock,
    },
}

struct PreparedBlock {
    block_id: [u8; 32],
    parent: [u8; 32],
    height: u64,
    accepted_at: u64,
    canonical: Vec<u8>,
    cumulative_work: U512,
    candidate: ValidatedCandidate,
}

pub fn devnet_params() -> Result<NetworkParams, NodeError> {
    let reference = v2_test_reference().map_err(PowError::from)?;
    let params = NetworkParams {
        network_id: DEVNET_NETWORK_ID,
        protocol_version: NETWORK_PROTOCOL_VERSION,
        genesis_hash: DEVNET_GENESIS_HASH,
        genesis_timestamp: DEVNET_GENESIS_TIMESTAMP,
        pow_limit: target_with_leading_zero_bits(8),
        pow: PowParameters::V2Reference(reference.descriptor()),
        monetary_policy: DEFAULT_MONETARY_POLICY,
        rewards: FixedRewardDestinations {
            // These deterministic keys are intentionally public and insecure.
            steward: insecure_dev_destination(0x11),
            community: insecure_dev_destination(0x12),
        },
        max_future_offset_secs: MAX_FUTURE_OFFSET_SECS,
    };
    params.validate()?;
    Ok(params)
}

pub fn default_miner_destination() -> [u8; 32] {
    insecure_dev_wallet_signing_key()
        .verifying_key()
        .to_bytes()
        .into()
}

pub fn parse_miner_destination(value: &str) -> Result<[u8; 32], NodeError> {
    if value.len() != 64 {
        return Err(NodeError::InvalidMinerDestination);
    }
    let bytes = hex::decode(value).map_err(|_| NodeError::InvalidMinerDestination)?;
    let destination: [u8; 32] = bytes
        .try_into()
        .map_err(|_| NodeError::InvalidMinerDestination)?;
    VerifyingKey::from_bytes(&destination).map_err(|_| NodeError::InvalidMinerDestination)?;
    Ok(destination)
}

pub fn unix_time_seconds() -> Result<u64, NodeError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| NodeError::InvalidSystemTime)?
        .as_secs())
}

impl Node {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, NodeError> {
        let data_dir = data_dir.as_ref().to_path_buf();
        let lock = DataDirLock::acquire(&data_dir)?;
        let params = devnet_params()?;
        let fingerprint = params.fingerprint()?;
        load_or_create_metadata(&data_dir, fingerprint)?;

        let reference = v2_test_reference().map_err(PowError::from)?;
        let verifier = ConsensusPowVerifier::v2_reference(reference);
        let mut state = ChainState::new(params, verifier.clone())?;
        let mut index = BlockIndex::new(params.genesis_hash);
        replay_log(
            &data_dir.join(BLOCK_LOG_FILE),
            &mut state,
            &mut index,
            &verifier,
            params,
            params.network_id,
        )?;
        let log_path = data_dir.join(BLOCK_LOG_FILE);
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&log_path)
            .map_err(|source| io_error("open block log", &log_path, source))?;

        Ok(Self {
            data_dir,
            params,
            fingerprint,
            verifier,
            state,
            index,
            mempool: BTreeMap::new(),
            mempool_bytes: 0,
            log,
            storage_faulted: false,
            _lock: lock,
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn status(&self) -> Result<NodeStatus, NodeError> {
        Ok(NodeStatus {
            network: "CommonFoundry Devnet-0",
            network_id: hex::encode(self.params.network_id),
            consensus_fingerprint: hex::encode(self.fingerprint),
            proof_of_work: "ForgeMatrix-v2 tiny full-recompute reference",
            tip: hex::encode(self.state.tip()),
            cumulative_work: hex::encode(chain_work_bytes(self.index.active_work)),
            accepted_height: self.state.next_height().saturating_sub(1),
            next_height: self.state.next_height(),
            expected_target: hex::encode(self.state.expected_target()?),
            utxo_count: self.state.utxos().len(),
            mempool_transactions: self.mempool.len(),
            mempool_bytes: self.mempool_bytes,
            storage_healthy: !self.storage_faulted,
        })
    }

    /// Returns the canonical Devnet-0 demo-wallet view.
    ///
    /// The wallet is intentionally fixed to the public development key. Its
    /// confirmed history is reconstructed from the active branch on every
    /// call, while balances come from the active UTXO set with the volatile
    /// mempool applied as a reservation/pending-output overlay.
    pub fn wallet_snapshot(&self) -> Result<WalletSnapshot, NodeError> {
        let destination = default_miner_destination();
        let reserved = mempool_spent_inputs(&self.mempool);
        let mut spendable = 0_u64;
        let mut immature = 0_u64;
        let mut spendable_utxo_count = 0_usize;
        let mut immature_utxo_count = 0_usize;
        let mut reserved_utxo_count = 0_usize;
        for (outpoint, output) in self.state.utxos().iter() {
            if output.lock != OutputLock::Key(destination) {
                continue;
            }
            if self.state.next_height() < output.spendable_height {
                immature = checked_wallet_add(immature, output.value)?;
                immature_utxo_count += 1;
            } else if reserved.contains(outpoint) {
                reserved_utxo_count += 1;
            } else {
                spendable = checked_wallet_add(spendable, output.value)?;
                spendable_utxo_count += 1;
            }
        }

        let mut pending = 0_u64;
        for entry in self.mempool.values() {
            for output in &entry.transaction.outputs {
                if output.lock == OutputLock::Key(destination) {
                    pending = checked_wallet_add(pending, output.value)?;
                }
            }
        }

        let accepted_height = self.state.next_height().saturating_sub(1);
        let mut history = self.pending_wallet_history(destination)?;
        let mut confirmed = self.confirmed_wallet_history(
            destination,
            accepted_height,
            MAX_WALLET_HISTORY.saturating_sub(history.len()),
        )?;
        confirmed.reverse();
        history.extend(confirmed);
        history.truncate(MAX_WALLET_HISTORY);

        Ok(WalletSnapshot {
            network: "CommonFoundry Devnet-0",
            devnet_only: true,
            insecure_demo_wallet: true,
            warning: DEV_WALLET_WARNING,
            destination: hex::encode(destination),
            accepted_height,
            next_height: self.state.next_height(),
            balances: WalletBalances {
                spendable_atoms: spendable.to_string(),
                immature_atoms: immature.to_string(),
                pending_atoms: pending.to_string(),
            },
            spendable_utxo_count,
            immature_utxo_count,
            reserved_utxo_count,
            mempool: WalletMempoolStatus {
                transactions: self.mempool.len(),
                bytes: self.mempool_bytes,
            },
            history_limit: MAX_WALLET_HISTORY,
            history,
        })
    }

    /// Creates and submits a payment from the intentionally public Devnet-0
    /// demo key. Selection is value-descending with a stable outpoint tie-break
    /// and never spends a mempool-reserved or immature output.
    pub fn send_from_dev_wallet(
        &mut self,
        recipient: [u8; 32],
        amount: u64,
        fee_burned: u64,
    ) -> Result<WalletSendResponse, NodeError> {
        VerifyingKey::from_bytes(&recipient).map_err(|_| NodeError::InvalidWalletRecipient)?;
        if amount == 0 {
            return Err(NodeError::WalletZeroAmount);
        }
        let required = amount
            .checked_add(fee_burned)
            .ok_or(NodeError::WalletAmountOverflow)?;
        let destination = default_miner_destination();
        let reserved = mempool_spent_inputs(&self.mempool);
        let mut candidates = Vec::new();
        let mut immature = 0_u64;
        for (outpoint, output) in self.state.utxos().iter() {
            if output.lock != OutputLock::Key(destination) {
                continue;
            }
            if self.state.next_height() < output.spendable_height {
                immature = checked_wallet_add(immature, output.value)?;
            } else if !reserved.contains(outpoint) {
                candidates.push((*outpoint, output.value));
            }
        }
        let selection = select_send_utxos(candidates, required)?;
        let available = selection.available;
        let selected = selection.outpoints;
        let selected_value = selection.selected_value;
        if selected_value < required {
            if available >= required {
                return Err(NodeError::WalletInputLimit {
                    selected: selected_value,
                    required,
                });
            }
            let available_with_immature = checked_wallet_add(available, immature)?;
            if available_with_immature >= required {
                return Err(NodeError::WalletFundsImmature { immature, required });
            }
            return Err(NodeError::WalletInsufficientFunds {
                available,
                required,
            });
        }

        let change = selected_value - required;
        let mut outputs = vec![TxOutput {
            value: amount,
            lock: OutputLock::Key(recipient),
            spendable_height: self.state.next_height(),
        }];
        if change > 0 {
            outputs.push(TxOutput {
                value: change,
                lock: OutputLock::Key(destination),
                spendable_height: self.state.next_height(),
            });
        }
        let mut transaction = Transaction {
            network_id: self.params.network_id,
            version: TRANSACTION_VERSION,
            inputs: selected
                .into_iter()
                .map(|previous| TxInput {
                    previous,
                    witness: InputWitness::Key {
                        public_key: [0; 32],
                        signature: Vec::new(),
                    },
                })
                .collect(),
            outputs,
        };
        let signing_key = insecure_dev_wallet_signing_key();
        let signing_keys = vec![&signing_key; transaction.inputs.len()];
        transaction.sign_all(&signing_keys)?;

        let encoded_bytes = encode_transaction(&transaction)?.len();
        let minimum_fee = required_relay_fee(encoded_bytes);
        if fee_burned < minimum_fee {
            return Err(NodeError::MempoolFeeTooLow {
                required: minimum_fee,
                actual: fee_burned,
            });
        }
        let entry = self.submit_transaction(transaction)?;
        Ok(WalletSendResponse {
            network: "CommonFoundry Devnet-0",
            devnet_only: true,
            insecure_demo_wallet: true,
            warning: DEV_WALLET_WARNING,
            txid: hex::encode(entry.txid),
            amount_atoms: amount.to_string(),
            fee_burned_atoms: entry.fee_burned.to_string(),
            change_atoms: change.to_string(),
            mempool_transactions: self.mempool.len(),
            mempool_bytes: self.mempool_bytes,
        })
    }

    /// Consolidates the smallest mature, unreserved demo-wallet outputs into
    /// exactly one immediately spendable self-owned output.
    pub fn consolidate_dev_wallet(
        &mut self,
        fee_burned: u64,
        max_inputs: usize,
    ) -> Result<WalletConsolidateResponse, NodeError> {
        if !(2..=MAX_TRANSACTION_INPUTS).contains(&max_inputs) {
            return Err(NodeError::InvalidConsolidationMaxInputs);
        }
        let destination = default_miner_destination();
        let reserved = mempool_spent_inputs(&self.mempool);
        let mut candidates: Vec<_> = self
            .state
            .utxos()
            .iter()
            .filter(|(outpoint, output)| {
                output.lock == OutputLock::Key(destination)
                    && self.state.next_height() >= output.spendable_height
                    && !reserved.contains(outpoint)
            })
            .map(|(outpoint, output)| (*outpoint, output.value))
            .collect();
        candidates.sort_unstable_by(|left, right| {
            left.1
                .cmp(&right.1)
                .then_with(|| left.0.txid.cmp(&right.0.txid))
                .then_with(|| left.0.index.cmp(&right.0.index))
        });
        let selected: Vec<_> = candidates.into_iter().take(max_inputs).collect();
        if selected.len() < 2 {
            return Err(NodeError::WalletNotEnoughUtxos(selected.len()));
        }
        let input_atoms = selected
            .iter()
            .try_fold(0_u64, |total, (_, value)| checked_wallet_add(total, *value))?;
        let output_atoms = input_atoms
            .checked_sub(fee_burned)
            .filter(|value| *value > 0)
            .ok_or(NodeError::WalletConsolidationFee { input: input_atoms })?;
        let mut transaction = Transaction {
            network_id: self.params.network_id,
            version: TRANSACTION_VERSION,
            inputs: selected
                .iter()
                .map(|(previous, _)| TxInput {
                    previous: *previous,
                    witness: InputWitness::Key {
                        public_key: [0; 32],
                        signature: Vec::new(),
                    },
                })
                .collect(),
            outputs: vec![TxOutput {
                value: output_atoms,
                lock: OutputLock::Key(destination),
                spendable_height: self.state.next_height(),
            }],
        };
        let signing_key = insecure_dev_wallet_signing_key();
        let signing_keys = vec![&signing_key; transaction.inputs.len()];
        transaction.sign_all(&signing_keys)?;
        let encoded_bytes = encode_transaction(&transaction)?.len();
        let minimum_fee = required_relay_fee(encoded_bytes);
        if fee_burned < minimum_fee {
            return Err(NodeError::MempoolFeeTooLow {
                required: minimum_fee,
                actual: fee_burned,
            });
        }
        let inputs_consolidated = transaction.inputs.len();
        let entry = self.submit_transaction(transaction)?;
        Ok(WalletConsolidateResponse {
            network: "CommonFoundry Devnet-0",
            devnet_only: true,
            insecure_demo_wallet: true,
            warning: DEV_WALLET_WARNING,
            txid: hex::encode(entry.txid),
            inputs_consolidated,
            input_atoms: input_atoms.to_string(),
            output_atoms: output_atoms.to_string(),
            fee_burned_atoms: entry.fee_burned.to_string(),
            mempool_transactions: self.mempool.len(),
            mempool_bytes: self.mempool_bytes,
        })
    }

    fn pending_wallet_history(
        &self,
        destination: [u8; 32],
    ) -> Result<Vec<WalletHistoryEntry>, NodeError> {
        let mut history = Vec::new();
        for entry in self.mempool.values().rev() {
            let mut wallet_inputs = 0_u64;
            let mut source = None;
            for input in &entry.transaction.inputs {
                let Some(previous) = self.state.utxos().get(&input.previous) else {
                    continue;
                };
                match previous.lock {
                    OutputLock::Key(owner) if owner == destination => {
                        wallet_inputs = checked_wallet_add(wallet_inputs, previous.value)?;
                    }
                    OutputLock::Key(owner) if source.is_none() => {
                        source = Some(hex::encode(owner));
                    }
                    _ => {}
                }
            }
            let mut wallet_outputs = 0_u64;
            let mut recipient = None;
            for output in &entry.transaction.outputs {
                match output.lock {
                    OutputLock::Key(owner) if owner == destination => {
                        wallet_outputs = checked_wallet_add(wallet_outputs, output.value)?;
                    }
                    OutputLock::Key(owner) if recipient.is_none() => {
                        recipient = Some(hex::encode(owner));
                    }
                    _ => {}
                }
            }
            let (kind, counterparty) = if wallet_inputs > 0 {
                ("sent", recipient)
            } else if wallet_outputs > 0 {
                ("received", source)
            } else {
                continue;
            };
            history.push(WalletHistoryEntry {
                kind,
                txid: hex::encode(entry.txid),
                height: None,
                timestamp: None,
                confirmations: 0,
                status: "pending",
                net_amount_atoms: signed_wallet_delta(wallet_outputs, wallet_inputs),
                fee_burned_atoms: entry.fee_burned.to_string(),
                counterparty,
            });
            if history.len() == MAX_WALLET_HISTORY {
                break;
            }
        }
        Ok(history)
    }

    fn confirmed_wallet_history(
        &self,
        destination: [u8; 32],
        accepted_height: u64,
        history_limit: usize,
    ) -> Result<Vec<WalletHistoryEntry>, NodeError> {
        let mut outputs = HashMap::<OutPoint, TxOutput>::new();
        let mut history = VecDeque::with_capacity(history_limit);
        for block_id in self.index.active_chain.iter().skip(1) {
            let indexed = self.index.blocks.get(block_id).ok_or_else(|| {
                NodeError::CorruptLog("active wallet history refers to an absent block".to_owned())
            })?;
            let block =
                decode_block(&indexed.canonical, self.params.network_id).map_err(|error| {
                    NodeError::CorruptLog(format!("active wallet history cannot decode: {error}"))
                })?;
            let height = block.challenge.height;
            let confirmations = accepted_height.saturating_sub(height).saturating_add(1);
            let coinbase_txid = block.coinbase_outpoint_id();
            let mut mined = 0_u64;
            let mut mined_is_immature = false;
            for (index, output) in block.coinbase.outputs.iter().enumerate() {
                let outpoint = OutPoint {
                    txid: coinbase_txid,
                    index: index as u32,
                };
                if output.lock == OutputLock::Key(destination) {
                    mined = checked_wallet_add(mined, output.value)?;
                    mined_is_immature |= self.state.next_height() < output.spendable_height;
                }
                outputs.insert(outpoint, output.clone());
            }
            if mined > 0 {
                retain_newest_wallet_history(
                    &mut history,
                    history_limit,
                    WalletHistoryEntry {
                        kind: "mined",
                        txid: hex::encode(coinbase_txid),
                        height: Some(height),
                        timestamp: Some(block.challenge.timestamp),
                        confirmations,
                        status: if mined_is_immature {
                            "immature"
                        } else {
                            "confirmed"
                        },
                        net_amount_atoms: mined.to_string(),
                        fee_burned_atoms: "0".to_owned(),
                        counterparty: None,
                    },
                );
            }

            for transaction in &block.transactions {
                let mut input_total = 0_u64;
                let mut wallet_inputs = 0_u64;
                let mut source = None;
                for input in &transaction.inputs {
                    let previous = outputs.remove(&input.previous).ok_or_else(|| {
                        NodeError::CorruptLog(
                            "active wallet history transaction refers to an absent output"
                                .to_owned(),
                        )
                    })?;
                    input_total = checked_wallet_add(input_total, previous.value)?;
                    match previous.lock {
                        OutputLock::Key(owner) if owner == destination => {
                            wallet_inputs = checked_wallet_add(wallet_inputs, previous.value)?;
                        }
                        OutputLock::Key(owner) if source.is_none() => {
                            source = Some(hex::encode(owner));
                        }
                        _ => {}
                    }
                }
                let txid = transaction.txid();
                let mut output_total = 0_u64;
                let mut wallet_outputs = 0_u64;
                let mut wallet_outputs_are_immature = false;
                let mut recipient = None;
                for (index, output) in transaction.outputs.iter().enumerate() {
                    output_total = checked_wallet_add(output_total, output.value)?;
                    match output.lock {
                        OutputLock::Key(owner) if owner == destination => {
                            wallet_outputs = checked_wallet_add(wallet_outputs, output.value)?;
                            wallet_outputs_are_immature |=
                                self.state.next_height() < output.spendable_height;
                        }
                        OutputLock::Key(owner) if recipient.is_none() => {
                            recipient = Some(hex::encode(owner));
                        }
                        _ => {}
                    }
                    outputs.insert(
                        OutPoint {
                            txid,
                            index: index as u32,
                        },
                        output.clone(),
                    );
                }
                let fee_burned = input_total.checked_sub(output_total).ok_or_else(|| {
                    NodeError::CorruptLog(
                        "active wallet history transaction creates value".to_owned(),
                    )
                })?;
                let (kind, counterparty) = if wallet_inputs > 0 {
                    ("sent", recipient)
                } else if wallet_outputs > 0 {
                    ("received", source)
                } else {
                    continue;
                };
                retain_newest_wallet_history(
                    &mut history,
                    history_limit,
                    WalletHistoryEntry {
                        kind,
                        txid: hex::encode(txid),
                        height: Some(height),
                        timestamp: Some(block.challenge.timestamp),
                        confirmations,
                        status: if wallet_outputs_are_immature {
                            "immature"
                        } else {
                            "confirmed"
                        },
                        net_amount_atoms: signed_wallet_delta(wallet_outputs, wallet_inputs),
                        fee_burned_atoms: fee_burned.to_string(),
                        counterparty,
                    },
                );
            }
        }
        Ok(history.into())
    }

    /// Builds the network- and consensus-bound peer greeting. This compatibility
    /// handshake does not authenticate a peer identity or encrypt the transport.
    pub fn peer_hello(&self) -> peer::PeerHello {
        peer::PeerHello {
            network_id: self.params.network_id,
            consensus_fingerprint: self.fingerprint,
            node_nonce: peer::process_node_nonce(),
            tip: self.state.tip(),
            height: self.state.next_height().saturating_sub(1),
            cumulative_work: self.cumulative_work(),
        }
    }

    pub fn cumulative_work(&self) -> peer::ChainWork {
        peer::ChainWork(chain_work_bytes(self.index.active_work))
    }

    /// Returns whether the identifier is known. The immutable virtual genesis
    /// is known even though it has no canonical block frame.
    pub fn contains_block(&self, block_id: [u8; 32]) -> bool {
        self.index.contains(block_id)
    }

    /// Returns the exact canonical frame retained for a validated block.
    /// Virtual genesis has no frame and therefore returns `None`.
    pub fn canonical_block(&self, block_id: [u8; 32]) -> Option<&[u8]> {
        self.index
            .blocks
            .get(&block_id)
            .map(|entry| entry.canonical.as_slice())
    }

    /// Builds a bounded, newest-first active-chain locator. For every nonzero
    /// bound the last identifier is virtual genesis; a zero bound is empty.
    pub fn block_locator(&self, max: usize) -> Vec<[u8; 32]> {
        if max == 0 {
            return Vec::new();
        }
        if max == 1 || self.index.active_chain.len() == 1 {
            return vec![self.index.genesis];
        }

        let mut locator = Vec::with_capacity(max.min(self.index.active_chain.len()));
        let mut position = self.index.active_chain.len() - 1;
        let mut step = 1_usize;
        while position > 0 && locator.len() + 1 < max {
            locator.push(self.index.active_chain[position]);
            position = position.saturating_sub(step);
            if locator.len() >= 10 {
                step = step.saturating_mul(2);
            }
        }
        locator.push(self.index.genesis);
        locator
    }

    /// Returns active-chain identifiers following the first locator entry that
    /// is on the active chain. `stop == [0; 32]` means no explicit stop; a
    /// nonzero stop is included when reached.
    pub fn inventory_after(
        &self,
        locator: &[[u8; 32]],
        stop: [u8; 32],
        max: usize,
    ) -> Vec<[u8; 32]> {
        if max == 0 {
            return Vec::new();
        }
        let Some(position) = locator
            .iter()
            .find_map(|block_id| self.index.active_position(*block_id))
        else {
            return Vec::new();
        };
        let remaining = self.index.active_chain.len().saturating_sub(position + 1);
        let mut inventory = Vec::with_capacity(max.min(remaining));
        for block_id in self.index.active_chain.iter().skip(position + 1) {
            inventory.push(*block_id);
            if inventory.len() == max || (stop != [0; 32] && *block_id == stop) {
                break;
            }
        }
        inventory
    }

    pub fn build_template(
        &self,
        miner_destination: [u8; 32],
        now_unix_seconds: u64,
    ) -> Result<BlockTemplate, NodeError> {
        if self.storage_faulted {
            return Err(NodeError::StorageFaulted);
        }
        VerifyingKey::from_bytes(&miner_destination)
            .map_err(|_| NodeError::InvalidMinerDestination)?;
        let earliest = self
            .state
            .median_time_past()
            .checked_add(1)
            .ok_or(NodeError::TemplateTimeUnavailable)?;
        let latest = now_unix_seconds
            .checked_add(self.params.max_future_offset_secs)
            .ok_or(NodeError::TemplateTimeUnavailable)?;
        let timestamp = now_unix_seconds.max(earliest);
        if timestamp > latest {
            return Err(NodeError::TemplateTimeUnavailable);
        }

        let height = self.state.next_height();
        let transactions: Vec<_> = self
            .mempool
            .values()
            .map(|entry| entry.transaction.clone())
            .collect();
        let validation = self
            .state
            .validate_transactions_for_next_block(&transactions)?;
        let allocation = self
            .params
            .monetary_policy
            .allocation(height, validation.total_burned_fees)?;
        let coinbase = Coinbase::new(height, allocation, miner_destination, self.params.rewards);
        let mut commitments = Vec::with_capacity(transactions.len() + 1);
        commitments.push(coinbase.commitment(self.params.network_id));
        commitments.extend(transactions.iter().map(Transaction::txid));
        let transaction_root = merkle_root(&commitments);
        let challenge = BlockChallenge {
            network_id: self.params.network_id,
            previous_block: self.state.tip(),
            transaction_root,
            height,
            timestamp,
            target: self.state.expected_target()?,
        };
        Ok(BlockTemplate {
            challenge,
            coinbase,
            transactions,
            total_fees_burned: validation.total_burned_fees,
        })
    }

    /// Adds a transaction to the volatile, active-chain-only Devnet-0 mempool.
    ///
    /// Unconfirmed parents are intentionally unsupported: every input must be
    /// present in the current active UTXO set. This keeps admission and reorg
    /// behavior deterministic while the test network has no package relay.
    pub fn submit_transaction(
        &mut self,
        transaction: Transaction,
    ) -> Result<MempoolEntry, NodeError> {
        let canonical = encode_transaction(&transaction)?;
        let txid = transaction.txid();
        if self.mempool.contains_key(&txid) {
            return Err(NodeError::DuplicateMempoolTransaction(txid));
        }
        if self.mempool.len() >= MAX_MEMPOOL_TRANSACTIONS {
            return Err(NodeError::MempoolTransactionLimit);
        }
        let next_bytes = self
            .mempool_bytes
            .checked_add(canonical.len())
            .ok_or(NodeError::MempoolByteLimit)?;
        if next_bytes > MAX_MEMPOOL_BYTES {
            return Err(NodeError::MempoolByteLimit);
        }

        let spent = mempool_spent_inputs(&self.mempool);
        let fee_burned =
            validate_mempool_transaction(&self.state, &transaction, canonical.len(), &spent)?;

        let mut ordered_transactions = Vec::with_capacity(self.mempool.len() + 1);
        let mut inserted = false;
        for (existing_txid, entry) in &self.mempool {
            if !inserted && txid < *existing_txid {
                ordered_transactions.push(transaction.clone());
                inserted = true;
            }
            ordered_transactions.push(entry.transaction.clone());
        }
        if !inserted {
            ordered_transactions.push(transaction.clone());
        }
        self.state
            .validate_transactions_for_next_block(&ordered_transactions)?;

        let entry = MempoolEntry {
            txid,
            transaction,
            encoded_bytes: canonical.len(),
            fee_burned,
        };
        self.mempool.insert(txid, entry.clone());
        self.mempool_bytes = next_bytes;
        Ok(entry)
    }

    pub fn mempool_entries(&self) -> impl ExactSizeIterator<Item = &MempoolEntry> {
        self.mempool.values()
    }

    pub fn mine_once(
        &mut self,
        miner_destination: [u8; 32],
        now_unix_seconds: u64,
        attempts: u64,
    ) -> Result<Block, NodeError> {
        let template = self.build_template(miner_destination, now_unix_seconds)?;
        let proof = self.verifier.mine(&template.challenge, 0, attempts)?;
        if !matches!(proof, BlockProof::V2Reference(_)) {
            return Err(NodeError::CorruptLog(
                "Devnet-0 verifier produced a non-v2 proof".to_owned(),
            ));
        }
        let block = Block {
            version: BLOCK_VERSION,
            challenge: template.challenge,
            proof,
            coinbase: template.coinbase,
            transactions: template.transactions,
        };
        self.submit_block(block.clone(), now_unix_seconds)?;
        Ok(block)
    }

    pub fn submit_block(&mut self, block: Block, accepted_at: u64) -> Result<u64, NodeError> {
        if self.storage_faulted {
            return Err(NodeError::StorageFaulted);
        }
        let previous_tip = self.state.tip();
        let confirmed_txids: HashSet<_> =
            block.transactions.iter().map(Transaction::txid).collect();
        let canonical = encode_block(&block)?;
        let prepared = prepare_block(
            &self.state,
            &self.index,
            self.params,
            &self.verifier,
            &block,
            accepted_at,
            canonical.clone(),
        )?;
        let record = encode_record(accepted_at, &canonical)?;
        let log_path = self.data_dir.join(BLOCK_LOG_FILE);
        if let Err(source) = self.log.write_all(&record) {
            self.storage_faulted = true;
            return Err(io_error("append block record", &log_path, source));
        }
        if let Err(source) = self.log.sync_all() {
            self.storage_faulted = true;
            return Err(io_error("sync block record", &log_path, source));
        }
        match commit_prepared(&mut self.state, &mut self.index, prepared) {
            Ok(fees) => {
                if self.state.tip() != previous_tip {
                    self.revalidate_mempool(&confirmed_txids);
                }
                Ok(fees)
            }
            Err(error) => {
                // The record is already durable. Refuse further work so a
                // restart can reconstruct the authoritative state from disk.
                self.storage_faulted = true;
                Err(error)
            }
        }
    }

    fn revalidate_mempool(&mut self, confirmed_txids: &HashSet<[u8; 32]>) {
        let old_pool = std::mem::take(&mut self.mempool);
        let mut retained = BTreeMap::new();
        let mut retained_transactions = Vec::new();
        let mut retained_spends = HashSet::new();
        let mut retained_bytes = 0_usize;

        for (txid, entry) in old_pool {
            if confirmed_txids.contains(&txid) {
                continue;
            }
            let Ok(fee_burned) = validate_mempool_transaction(
                &self.state,
                &entry.transaction,
                entry.encoded_bytes,
                &retained_spends,
            ) else {
                continue;
            };
            retained_transactions.push(entry.transaction.clone());
            if self
                .state
                .validate_transactions_for_next_block(&retained_transactions)
                .is_err()
            {
                retained_transactions.pop();
                continue;
            }
            retained_spends.extend(entry.transaction.inputs.iter().map(|input| input.previous));
            retained_bytes += entry.encoded_bytes;
            retained.insert(
                txid,
                MempoolEntry {
                    fee_burned,
                    ..entry
                },
            );
        }

        self.mempool = retained;
        self.mempool_bytes = retained_bytes;
    }
}

fn mempool_spent_inputs(mempool: &BTreeMap<[u8; 32], MempoolEntry>) -> HashSet<OutPoint> {
    mempool
        .values()
        .flat_map(|entry| entry.transaction.inputs.iter().map(|input| input.previous))
        .collect()
}

fn validate_mempool_transaction(
    state: &ChainState,
    transaction: &Transaction,
    encoded_bytes: usize,
    mempool_spends: &HashSet<OutPoint>,
) -> Result<u64, NodeError> {
    for input in &transaction.inputs {
        if state.utxos().get(&input.previous).is_none() {
            return Err(NodeError::MempoolUnconfirmedInput(input.previous));
        }
        if mempool_spends.contains(&input.previous) {
            return Err(NodeError::MempoolInputConflict(input.previous));
        }
    }
    let validation =
        state.validate_transactions_for_next_block(std::slice::from_ref(transaction))?;
    let required = required_relay_fee(encoded_bytes);
    if validation.total_burned_fees < required {
        return Err(NodeError::MempoolFeeTooLow {
            required,
            actual: validation.total_burned_fees,
        });
    }
    Ok(validation.total_burned_fees)
}

fn required_relay_fee(encoded_bytes: usize) -> u64 {
    let kib = encoded_bytes.saturating_add(1023) / 1024;
    u64::try_from(kib)
        .ok()
        .and_then(|kib| kib.checked_mul(MIN_RELAY_FEE_PER_KIB))
        .unwrap_or(u64::MAX)
        .max(MIN_RELAY_FEE_PER_KIB)
}

fn checked_wallet_add(left: u64, right: u64) -> Result<u64, NodeError> {
    left.checked_add(right)
        .ok_or(NodeError::WalletAmountOverflow)
}

#[derive(Debug, PartialEq, Eq)]
struct WalletCoinSelection {
    outpoints: Vec<OutPoint>,
    selected_value: u64,
    available: u64,
}

fn select_send_utxos(
    mut candidates: Vec<(OutPoint, u64)>,
    required: u64,
) -> Result<WalletCoinSelection, NodeError> {
    let available = candidates
        .iter()
        .try_fold(0_u64, |total, (_, value)| checked_wallet_add(total, *value))?;
    candidates.sort_unstable_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then_with(|| left.0.txid.cmp(&right.0.txid))
            .then_with(|| left.0.index.cmp(&right.0.index))
    });
    let mut outpoints = Vec::new();
    let mut selected_value = 0_u64;
    for (outpoint, value) in candidates.into_iter().take(MAX_TRANSACTION_INPUTS) {
        outpoints.push(outpoint);
        selected_value = checked_wallet_add(selected_value, value)?;
        if selected_value >= required {
            break;
        }
    }
    Ok(WalletCoinSelection {
        outpoints,
        selected_value,
        available,
    })
}

fn signed_wallet_delta(credits: u64, debits: u64) -> String {
    if credits >= debits {
        (credits - debits).to_string()
    } else {
        format!("-{}", debits - credits)
    }
}

fn retain_newest_wallet_history(
    history: &mut VecDeque<WalletHistoryEntry>,
    limit: usize,
    entry: WalletHistoryEntry,
) {
    if limit == 0 {
        return;
    }
    if history.len() == limit {
        history.pop_front();
    }
    history.push_back(entry);
}

fn prepare_block(
    active_state: &ChainState,
    index: &BlockIndex,
    params: NetworkParams,
    verifier: &ConsensusPowVerifier,
    block: &Block,
    accepted_at: u64,
    canonical: Vec<u8>,
) -> Result<PreparedBlock, NodeError> {
    let block_id = block.block_id();
    if index.contains(block_id) {
        return Err(NodeError::DuplicateBlock(block_id));
    }
    let parent = block.challenge.previous_block;
    let parent_work = index
        .work_at(parent)
        .ok_or(NodeError::UnknownParent(parent))?;
    let context = BlockValidationContext {
        now_unix_seconds: accepted_at,
    };
    let candidate = if parent == active_state.tip() {
        ValidatedCandidate::Active(active_state.validate_block(block, context)?)
    } else {
        // Devnet-0 intentionally replays the complete side branch here. This
        // keeps fork validation simple and prevents an unvalidated header-only
        // branch from entering the index or influencing fork choice.
        let branch_state = rebuild_state_to(index, params, verifier, parent)?;
        let validated = branch_state.validate_block(block, context)?;
        ValidatedCandidate::Branch {
            state: Box::new(branch_state),
            validated,
        }
    };
    let cumulative_work =
        add_chain_work(parent_work, block.challenge.target).map_err(ChainError::from)?;
    Ok(PreparedBlock {
        block_id,
        parent,
        height: block.challenge.height,
        accepted_at,
        canonical,
        cumulative_work,
        candidate,
    })
}

fn rebuild_state_to(
    index: &BlockIndex,
    params: NetworkParams,
    verifier: &ConsensusPowVerifier,
    tip: [u8; 32],
) -> Result<ChainState, NodeError> {
    let mut state = ChainState::new(params, verifier.clone())?;
    for block_id in index.path_to(tip)? {
        let entry = index.blocks.get(&block_id).ok_or_else(|| {
            NodeError::CorruptLog("fork path refers to an absent block".to_owned())
        })?;
        let block = decode_block(&entry.canonical, params.network_id).map_err(|error| {
            NodeError::CorruptLog(format!("indexed block cannot decode: {error}"))
        })?;
        if block.block_id() != block_id
            || block.challenge.previous_block != entry.parent
            || block.challenge.height != entry.height
        {
            return Err(NodeError::CorruptLog(
                "indexed block metadata does not match its canonical frame".to_owned(),
            ));
        }
        state
            .validate_and_apply(
                &block,
                BlockValidationContext {
                    now_unix_seconds: entry.accepted_at,
                },
            )
            .map_err(|error| {
                NodeError::CorruptLog(format!(
                    "indexed side branch fails full consensus replay: {error}"
                ))
            })?;
    }
    Ok(state)
}

fn commit_prepared(
    active_state: &mut ChainState,
    index: &mut BlockIndex,
    prepared: PreparedBlock,
) -> Result<u64, NodeError> {
    let activates = prepared.cumulative_work > index.active_work;
    if matches!(prepared.candidate, ValidatedCandidate::Active(_)) && !activates {
        return Err(NodeError::CorruptLog(
            "active extension did not increase cumulative work".to_owned(),
        ));
    }
    let next_active_chain = if activates {
        let mut chain = Vec::new();
        chain.push(index.genesis);
        chain.extend(index.path_to(prepared.parent)?);
        chain.push(prepared.block_id);
        Some(chain)
    } else {
        None
    };

    let fees = match prepared.candidate {
        ValidatedCandidate::Active(validated) => active_state.commit_validated(validated)?,
        ValidatedCandidate::Branch {
            mut state,
            validated,
        } => {
            let fees = state.commit_validated(validated)?;
            if activates {
                *active_state = *state;
            }
            fees
        }
    };

    let previous = index.blocks.insert(
        prepared.block_id,
        IndexedBlock {
            parent: prepared.parent,
            height: prepared.height,
            accepted_at: prepared.accepted_at,
            canonical: prepared.canonical,
            cumulative_work: prepared.cumulative_work,
        },
    );
    if previous.is_some() {
        return Err(NodeError::DuplicateBlock(prepared.block_id));
    }
    if let Some(active_chain) = next_active_chain {
        index.active_chain = active_chain;
        index.active_work = prepared.cumulative_work;
    }
    Ok(fees)
}

#[derive(Debug)]
struct RpcRequest {
    method: String,
    target: String,
    content_type: Option<String>,
    body: Vec<u8>,
}

struct DeadlineReader<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}

impl<'a> DeadlineReader<'a> {
    fn new(stream: &'a mut TcpStream, total_timeout: Duration) -> Self {
        Self {
            stream,
            deadline: Instant::now() + total_timeout,
        }
    }
}

impl Read for DeadlineReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "RPC deadline exceeded"))?;
        self.stream
            .set_read_timeout(Some(remaining.min(RPC_READ_TIMEOUT)))?;
        self.stream.read(buffer)
    }
}

struct RpcResponse {
    status: u16,
    reason: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

impl RpcResponse {
    fn json(status: u16, reason: &'static str, value: serde_json::Value) -> Self {
        Self {
            status,
            reason,
            content_type: "application/json",
            body: serde_json::to_vec(&value).expect("JSON value serialization cannot fail"),
        }
    }

    fn json_error(status: u16, reason: &'static str, error: impl ToString) -> Self {
        Self::json(status, reason, json!({ "error": error.to_string() }))
    }
}

/// Runs the intentionally small, single-threaded Devnet-0 RPC service.
///
/// The listener refuses non-loopback addresses. Each connection has fixed
/// timeouts and request-size limits, and is closed after one HTTP/1.1 request.
pub fn serve_rpc(node: Node, bind: SocketAddr) -> Result<(), NodeError> {
    serve_rpc_shared(Arc::new(Mutex::new(node)), bind)
}

/// Runs the bounded RPC service against a node shared with the P2P runtime.
pub fn serve_rpc_shared(shared: Arc<Mutex<Node>>, bind: SocketAddr) -> Result<(), NodeError> {
    if !bind.ip().is_loopback() {
        return Err(NodeError::NonLoopbackRpc(bind));
    }
    let listener = TcpListener::bind(bind)
        .map_err(|source| io_error("bind RPC listener", PathBuf::from(bind.to_string()), source))?;
    for connection in listener.incoming() {
        let mut stream = match connection {
            Ok(stream) => stream,
            Err(_) => continue,
        };
        if let Err(error) = handle_rpc_connection_shared(&mut stream, &shared) {
            let response = RpcResponse::json_error(500, "Internal Server Error", error);
            let _ = write_rpc_response(&mut stream, response);
        }
    }
    Ok(())
}

fn handle_rpc_connection_shared(
    stream: &mut TcpStream,
    shared: &Arc<Mutex<Node>>,
) -> Result<(), NodeError> {
    stream
        .set_read_timeout(Some(RPC_READ_TIMEOUT))
        .map_err(NodeError::RpcIo)?;
    stream
        .set_write_timeout(Some(RPC_WRITE_TIMEOUT))
        .map_err(NodeError::RpcIo)?;
    let mut deadline_reader = DeadlineReader::new(stream, RPC_TOTAL_READ_TIMEOUT);
    let request = match read_rpc_request(&mut deadline_reader) {
        Ok(request) => request,
        Err(error) => {
            return write_rpc_response(stream, RpcResponse::json_error(400, "Bad Request", error));
        }
    };
    let response = {
        let mut node = shared.lock().map_err(|_| NodeError::SharedNodePoisoned)?;
        route_rpc_request(request, &mut node)
    };
    write_rpc_response(stream, response)
}

fn route_rpc_request(request: RpcRequest, node: &mut Node) -> RpcResponse {
    match (request.method.as_str(), request.target.as_str()) {
        ("GET", "/health") if node.storage_faulted => RpcResponse::json(
            503,
            "Service Unavailable",
            json!({
                "ok": false,
                "storage_healthy": false,
                "network": "CommonFoundry Devnet-0"
            }),
        ),
        ("GET", "/health") => RpcResponse::json(
            200,
            "OK",
            json!({
                "ok": true,
                "storage_healthy": true,
                "network": "CommonFoundry Devnet-0"
            }),
        ),
        ("GET", "/v1/status") => match node.status() {
            Ok(status) => match serde_json::to_value(status) {
                Ok(value) => RpcResponse::json(200, "OK", value),
                Err(error) => RpcResponse::json_error(500, "Internal Server Error", error),
            },
            Err(error) => RpcResponse::json_error(500, "Internal Server Error", error),
        },
        ("GET", "/v1/mempool") => RpcResponse::json(200, "OK", mempool_json(node)),
        ("GET", "/v1/wallet") => match node.wallet_snapshot() {
            Ok(snapshot) => match serde_json::to_value(snapshot) {
                Ok(value) => RpcResponse::json(200, "OK", value),
                Err(error) => RpcResponse::json_error(500, "Internal Server Error", error),
            },
            Err(error) => RpcResponse::json_error(500, "Internal Server Error", error),
        },
        ("POST", "/v1/wallet/send") => {
            if !has_json_content_type(request.content_type.as_deref()) {
                return RpcResponse::json_error(
                    415,
                    "Unsupported Media Type",
                    "Content-Type must be application/json",
                );
            }
            let send: WalletSendRequest = match serde_json::from_slice(&request.body) {
                Ok(send) => send,
                Err(error) => return RpcResponse::json_error(400, "Bad Request", error),
            };
            let recipient = match parse_wallet_recipient(&send.recipient) {
                Ok(recipient) => recipient,
                Err(error) => return RpcResponse::json_error(400, "Bad Request", error),
            };
            let amount = match parse_cmfd_atoms(&send.amount) {
                Ok(0) => {
                    return RpcResponse::json_error(
                        400,
                        "Bad Request",
                        NodeError::WalletZeroAmount,
                    );
                }
                Ok(amount) => amount,
                Err(error) => return RpcResponse::json_error(400, "Bad Request", error),
            };
            let fee = match parse_cmfd_atoms(&send.fee) {
                Ok(fee) => fee,
                Err(error) => return RpcResponse::json_error(400, "Bad Request", error),
            };
            match node.send_from_dev_wallet(recipient, amount, fee) {
                Ok(response) => match serde_json::to_value(response) {
                    Ok(value) => RpcResponse::json(200, "OK", value),
                    Err(error) => RpcResponse::json_error(500, "Internal Server Error", error),
                },
                Err(
                    error @ (NodeError::DuplicateMempoolTransaction(_)
                    | NodeError::MempoolInputConflict(_)),
                ) => RpcResponse::json_error(409, "Conflict", error),
                Err(
                    error @ (NodeError::WalletFundsImmature { .. }
                    | NodeError::WalletInsufficientFunds { .. }
                    | NodeError::WalletInputLimit { .. }
                    | NodeError::MempoolUnconfirmedInput(_)
                    | NodeError::MempoolTransactionLimit
                    | NodeError::MempoolByteLimit
                    | NodeError::MempoolFeeTooLow { .. }
                    | NodeError::Chain(_)),
                ) => RpcResponse::json_error(422, "Unprocessable Content", error),
                Err(
                    error @ (NodeError::InvalidWalletRecipient
                    | NodeError::InvalidWalletAmount
                    | NodeError::WalletAmountOverflow
                    | NodeError::WalletZeroAmount),
                ) => RpcResponse::json_error(400, "Bad Request", error),
                Err(error) => RpcResponse::json_error(500, "Internal Server Error", error),
            }
        }
        ("POST", "/v1/wallet/consolidate") => {
            if !has_json_content_type(request.content_type.as_deref()) {
                return RpcResponse::json_error(
                    415,
                    "Unsupported Media Type",
                    "Content-Type must be application/json",
                );
            }
            let consolidation: WalletConsolidateRequest =
                match serde_json::from_slice(&request.body) {
                    Ok(consolidation) => consolidation,
                    Err(error) => return RpcResponse::json_error(400, "Bad Request", error),
                };
            if !(2..=MAX_TRANSACTION_INPUTS).contains(&consolidation.max_inputs) {
                return RpcResponse::json_error(
                    400,
                    "Bad Request",
                    NodeError::InvalidConsolidationMaxInputs,
                );
            }
            let fee = match parse_cmfd_atoms(&consolidation.fee) {
                Ok(fee) => fee,
                Err(error) => return RpcResponse::json_error(400, "Bad Request", error),
            };
            match node.consolidate_dev_wallet(fee, consolidation.max_inputs) {
                Ok(response) => match serde_json::to_value(response) {
                    Ok(value) => RpcResponse::json(200, "OK", value),
                    Err(error) => RpcResponse::json_error(500, "Internal Server Error", error),
                },
                Err(
                    error @ (NodeError::DuplicateMempoolTransaction(_)
                    | NodeError::MempoolInputConflict(_)),
                ) => RpcResponse::json_error(409, "Conflict", error),
                Err(
                    error @ (NodeError::WalletNotEnoughUtxos(_)
                    | NodeError::WalletConsolidationFee { .. }
                    | NodeError::MempoolUnconfirmedInput(_)
                    | NodeError::MempoolTransactionLimit
                    | NodeError::MempoolByteLimit
                    | NodeError::MempoolFeeTooLow { .. }
                    | NodeError::Chain(_)),
                ) => RpcResponse::json_error(422, "Unprocessable Content", error),
                Err(
                    error @ (NodeError::InvalidConsolidationMaxInputs
                    | NodeError::InvalidWalletAmount
                    | NodeError::WalletAmountOverflow),
                ) => RpcResponse::json_error(400, "Bad Request", error),
                Err(error) => RpcResponse::json_error(500, "Internal Server Error", error),
            }
        }
        ("GET", target) if target.starts_with("/v1/template?") => {
            let miner = match parse_template_miner(target) {
                Ok(miner) => miner,
                Err(error) => return RpcResponse::json_error(400, "Bad Request", error),
            };
            let now = match unix_time_seconds() {
                Ok(now) => now,
                Err(error) => {
                    return RpcResponse::json_error(500, "Internal Server Error", error);
                }
            };
            match node.build_template(miner, now) {
                Ok(template) => RpcResponse::json(200, "OK", template_json(&template)),
                Err(NodeError::StorageFaulted) => {
                    RpcResponse::json_error(503, "Service Unavailable", NodeError::StorageFaulted)
                }
                Err(error) => RpcResponse::json_error(422, "Unprocessable Content", error),
            }
        }
        ("POST", "/v1/transaction") => {
            if !has_octet_stream_content_type(request.content_type.as_deref()) {
                return RpcResponse::json_error(
                    415,
                    "Unsupported Media Type",
                    "Content-Type must be application/octet-stream",
                );
            }
            let transaction = match decode_transaction(&request.body, DEVNET_NETWORK_ID) {
                Ok(transaction) => transaction,
                Err(error) => return RpcResponse::json_error(400, "Bad Request", error),
            };
            match encode_transaction(&transaction) {
                Ok(canonical) if canonical == request.body => {}
                Ok(_) => {
                    return RpcResponse::json_error(
                        400,
                        "Bad Request",
                        "transaction frame is not canonical",
                    );
                }
                Err(error) => return RpcResponse::json_error(400, "Bad Request", error),
            }
            match node.submit_transaction(transaction) {
                Ok(entry) => RpcResponse::json(
                    200,
                    "OK",
                    json!({
                        "accepted": true,
                        "txid": hex::encode(entry.txid),
                        "encoded_bytes": entry.encoded_bytes,
                        "fee_burned": entry.fee_burned,
                        "mempool_transactions": node.mempool.len(),
                        "mempool_bytes": node.mempool_bytes,
                    }),
                ),
                Err(
                    error @ (NodeError::DuplicateMempoolTransaction(_)
                    | NodeError::MempoolInputConflict(_)),
                ) => RpcResponse::json_error(409, "Conflict", error),
                Err(
                    error @ (NodeError::MempoolUnconfirmedInput(_)
                    | NodeError::MempoolTransactionLimit
                    | NodeError::MempoolByteLimit
                    | NodeError::MempoolFeeTooLow { .. }
                    | NodeError::Chain(_)),
                ) => RpcResponse::json_error(422, "Unprocessable Content", error),
                Err(error) => RpcResponse::json_error(500, "Internal Server Error", error),
            }
        }
        ("POST", target) if target.starts_with("/v1/mine?") => {
            if !request.body.is_empty() {
                return RpcResponse::json_error(
                    400,
                    "Bad Request",
                    "mine endpoint requires an empty body",
                );
            }
            let (miner, attempts) = match parse_mine_request(target) {
                Ok(values) => values,
                Err(error) => return RpcResponse::json_error(400, "Bad Request", error),
            };
            let now = match unix_time_seconds() {
                Ok(now) => now,
                Err(error) => {
                    return RpcResponse::json_error(500, "Internal Server Error", error);
                }
            };
            match node.mine_once(miner, now, attempts) {
                Ok(block) => match node.status() {
                    Ok(status) => RpcResponse::json(
                        200,
                        "OK",
                        json!({
                            "accepted": true,
                            "block_id": hex::encode(block.block_id()),
                            "height": status.accepted_height,
                            "tip": status.tip,
                        }),
                    ),
                    Err(error) => RpcResponse::json_error(500, "Internal Server Error", error),
                },
                Err(NodeError::StorageFaulted) => {
                    RpcResponse::json_error(503, "Service Unavailable", NodeError::StorageFaulted)
                }
                Err(error @ (NodeError::Pow(_) | NodeError::Chain(_))) => {
                    RpcResponse::json_error(422, "Unprocessable Content", error)
                }
                Err(error) => RpcResponse::json_error(500, "Internal Server Error", error),
            }
        }
        ("POST", "/v1/block") => {
            if !has_octet_stream_content_type(request.content_type.as_deref()) {
                return RpcResponse::json_error(
                    415,
                    "Unsupported Media Type",
                    "Content-Type must be application/octet-stream",
                );
            }
            let block = match decode_block(&request.body, DEVNET_NETWORK_ID) {
                Ok(block) => block,
                Err(error) => return RpcResponse::json_error(400, "Bad Request", error),
            };
            match encode_block(&block) {
                Ok(canonical) if canonical == request.body => {}
                Ok(_) => {
                    return RpcResponse::json_error(
                        400,
                        "Bad Request",
                        "block frame is not canonical",
                    );
                }
                Err(error) => return RpcResponse::json_error(400, "Bad Request", error),
            }
            let accepted_at = match unix_time_seconds() {
                Ok(accepted_at) => accepted_at,
                Err(error) => {
                    return RpcResponse::json_error(500, "Internal Server Error", error);
                }
            };
            match node.submit_block(block, accepted_at) {
                Ok(fees_burned) => match node.status() {
                    Ok(status) => RpcResponse::json(
                        200,
                        "OK",
                        json!({
                            "accepted": true,
                            "height": status.accepted_height,
                            "tip": status.tip,
                            "fees_burned": fees_burned,
                        }),
                    ),
                    Err(error) => RpcResponse::json_error(500, "Internal Server Error", error),
                },
                Err(NodeError::Chain(error)) => {
                    RpcResponse::json_error(422, "Unprocessable Content", error)
                }
                Err(error) => RpcResponse::json_error(500, "Internal Server Error", error),
            }
        }
        ("GET" | "POST", _) => RpcResponse::json_error(404, "Not Found", "unknown endpoint"),
        _ => RpcResponse::json_error(405, "Method Not Allowed", "method not allowed"),
    }
}

fn has_octet_stream_content_type(content_type: Option<&str>) -> bool {
    content_type
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| {
            value
                .trim()
                .eq_ignore_ascii_case("application/octet-stream")
        })
}

fn has_json_content_type(content_type: Option<&str>) -> bool {
    content_type
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

fn parse_wallet_recipient(value: &str) -> Result<[u8; 32], NodeError> {
    if value.len() != 64 {
        return Err(NodeError::InvalidWalletRecipient);
    }
    let bytes = hex::decode(value).map_err(|_| NodeError::InvalidWalletRecipient)?;
    let recipient: [u8; 32] = bytes
        .try_into()
        .map_err(|_| NodeError::InvalidWalletRecipient)?;
    VerifyingKey::from_bytes(&recipient).map_err(|_| NodeError::InvalidWalletRecipient)?;
    Ok(recipient)
}

fn parse_cmfd_atoms(value: &str) -> Result<u64, NodeError> {
    let mut parts = value.split('.');
    let whole = parts.next().ok_or(NodeError::InvalidWalletAmount)?;
    let fractional = parts.next();
    if parts.next().is_some()
        || whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(NodeError::InvalidWalletAmount);
    }
    let whole_atoms = whole
        .parse::<u64>()
        .map_err(|_| NodeError::WalletAmountOverflow)?
        .checked_mul(COIN)
        .ok_or(NodeError::WalletAmountOverflow)?;
    let fractional_atoms = match fractional {
        None => 0,
        Some(digits)
            if !digits.is_empty()
                && digits.len() <= 8
                && digits.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            let value = digits
                .parse::<u64>()
                .map_err(|_| NodeError::InvalidWalletAmount)?;
            value * 10_u64.pow(8 - digits.len() as u32)
        }
        Some(_) => return Err(NodeError::InvalidWalletAmount),
    };
    whole_atoms
        .checked_add(fractional_atoms)
        .ok_or(NodeError::WalletAmountOverflow)
}

fn parse_template_miner(target: &str) -> Result<[u8; 32], NodeError> {
    let (path, query) = target
        .split_once('?')
        .ok_or_else(|| NodeError::InvalidRpcRequest("template requires miner query".to_owned()))?;
    if path != "/v1/template" || query.contains('&') {
        return Err(NodeError::InvalidRpcRequest(
            "template accepts exactly one miner query".to_owned(),
        ));
    }
    let value = query
        .strip_prefix("miner=")
        .ok_or_else(|| NodeError::InvalidRpcRequest("template requires miner query".to_owned()))?;
    parse_miner_destination(value)
}

fn parse_mine_request(target: &str) -> Result<([u8; 32], u64), NodeError> {
    let (path, query) = target
        .split_once('?')
        .ok_or_else(|| NodeError::InvalidRpcRequest("mine requires query parameters".to_owned()))?;
    if path != "/v1/mine" {
        return Err(NodeError::InvalidRpcRequest(
            "invalid mine endpoint".to_owned(),
        ));
    }

    let mut miner = None;
    let mut attempts = None;
    for parameter in query.split('&') {
        let (name, value) = parameter.split_once('=').ok_or_else(|| {
            NodeError::InvalidRpcRequest("malformed mine query parameter".to_owned())
        })?;
        match name {
            "miner" if miner.is_none() => miner = Some(parse_miner_destination(value)?),
            "attempts" if attempts.is_none() => {
                let parsed = value.parse::<u64>().map_err(|_| {
                    NodeError::InvalidRpcRequest("mine attempts must be an integer".to_owned())
                })?;
                if parsed == 0 || parsed > DEFAULT_MINING_ATTEMPTS {
                    return Err(NodeError::InvalidRpcRequest(format!(
                        "mine attempts must be between 1 and {DEFAULT_MINING_ATTEMPTS}"
                    )));
                }
                attempts = Some(parsed);
            }
            _ => {
                return Err(NodeError::InvalidRpcRequest(
                    "mine requires exactly one miner and one attempts parameter".to_owned(),
                ));
            }
        }
    }
    let miner = miner.ok_or_else(|| {
        NodeError::InvalidRpcRequest("mine requires a miner parameter".to_owned())
    })?;
    let attempts = attempts.ok_or_else(|| {
        NodeError::InvalidRpcRequest("mine requires an attempts parameter".to_owned())
    })?;
    Ok((miner, attempts))
}

fn template_json(template: &BlockTemplate) -> serde_json::Value {
    let outputs: Vec<_> = template
        .coinbase
        .outputs
        .iter()
        .map(|output| {
            let destination = match output.lock {
                OutputLock::Key(destination) => hex::encode(destination),
                OutputLock::InferenceChannel { channel_id } => hex::encode(channel_id),
            };
            json!({
                "value": output.value,
                "destination": destination,
                "spendable_height": output.spendable_height,
            })
        })
        .collect();
    json!({
        "network": "CommonFoundry Devnet-0",
        "proof_type": "forgematrix-v2-reference",
        "block_version": BLOCK_VERSION,
        "network_id": hex::encode(template.challenge.network_id),
        "previous_block": hex::encode(template.challenge.previous_block),
        "transaction_root": hex::encode(template.challenge.transaction_root),
        "height": template.challenge.height,
        "timestamp": template.challenge.timestamp,
        "target": hex::encode(template.challenge.target),
        "coinbase": {
            "height": template.coinbase.height,
            "outputs": outputs,
        },
        "transactions": template.transactions,
        "transaction_ids": template
            .transactions
            .iter()
            .map(|transaction| hex::encode(transaction.txid()))
            .collect::<Vec<_>>(),
        "fees_burned": template.total_fees_burned,
    })
}

fn mempool_json(node: &Node) -> serde_json::Value {
    let entries: Vec<_> = node
        .mempool
        .values()
        .map(|entry| {
            json!({
                "txid": hex::encode(entry.txid),
                "encoded_bytes": entry.encoded_bytes,
                "fee_burned": entry.fee_burned,
                "fee_burned_atoms": entry.fee_burned.to_string(),
            })
        })
        .collect();
    json!({
        "transactions": node.mempool.len(),
        "bytes": node.mempool_bytes,
        "entries": entries,
    })
}

fn read_rpc_request(reader: &mut impl Read) -> Result<RpcRequest, NodeError> {
    let mut bytes = Vec::new();
    let header_end = loop {
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let end = position + 4;
            if end > RPC_HEADER_LIMIT {
                return Err(NodeError::InvalidRpcRequest(
                    "headers exceed 8192 bytes".to_owned(),
                ));
            }
            break end;
        }
        if bytes.len() >= RPC_HEADER_LIMIT {
            return Err(NodeError::InvalidRpcRequest(
                "headers exceed 8192 bytes".to_owned(),
            ));
        }
        let mut chunk = [0_u8; 1024];
        let count = reader.read(&mut chunk).map_err(NodeError::RpcIo)?;
        if count == 0 {
            return Err(NodeError::InvalidRpcRequest(
                "request ended before headers were complete".to_owned(),
            ));
        }
        bytes.extend_from_slice(&chunk[..count]);
    };

    let header = std::str::from_utf8(&bytes[..header_end - 4])
        .map_err(|_| NodeError::InvalidRpcRequest("headers are not UTF-8".to_owned()))?;
    let mut lines = header.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| NodeError::InvalidRpcRequest("missing request line".to_owned()))?;
    let request_parts: Vec<_> = request_line.split_whitespace().collect();
    if request_parts.len() != 3 || request_parts[2] != "HTTP/1.1" {
        return Err(NodeError::InvalidRpcRequest(
            "request line must use HTTP/1.1".to_owned(),
        ));
    }

    let mut content_length = None;
    let mut content_type = None;
    let body_limit = rpc_body_limit(request_parts[0], request_parts[1]);
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| NodeError::InvalidRpcRequest("malformed HTTP header".to_owned()))?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(NodeError::InvalidRpcRequest(
                    "duplicate Content-Length".to_owned(),
                ));
            }
            let length = value
                .parse::<usize>()
                .map_err(|_| NodeError::InvalidRpcRequest("invalid Content-Length".to_owned()))?;
            if length > body_limit {
                return Err(NodeError::InvalidRpcRequest(format!(
                    "request body exceeds the {body_limit}-byte endpoint limit"
                )));
            }
            content_length = Some(length);
        } else if name.eq_ignore_ascii_case("content-type") {
            content_type = Some(value.to_owned());
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(NodeError::InvalidRpcRequest(
                "Transfer-Encoding is not supported".to_owned(),
            ));
        }
    }

    let body_length = content_length.unwrap_or(0);
    if request_parts[0] == "POST" && content_length.is_none() {
        return Err(NodeError::InvalidRpcRequest(
            "POST requires Content-Length".to_owned(),
        ));
    }
    let mut body = bytes[header_end..].to_vec();
    if body.len() > body_length {
        return Err(NodeError::InvalidRpcRequest(
            "request contains bytes after its declared body".to_owned(),
        ));
    }
    let remaining = body_length - body.len();
    if remaining > 0 {
        let original_len = body.len();
        body.resize(body_length, 0);
        reader
            .read_exact(&mut body[original_len..])
            .map_err(|error| {
                if error.kind() == io::ErrorKind::UnexpectedEof {
                    NodeError::InvalidRpcRequest("request body is truncated".to_owned())
                } else {
                    NodeError::RpcIo(error)
                }
            })?;
    }
    if request_parts[0] == "GET" && !body.is_empty() {
        return Err(NodeError::InvalidRpcRequest(
            "GET requests may not contain a body".to_owned(),
        ));
    }

    Ok(RpcRequest {
        method: request_parts[0].to_owned(),
        target: request_parts[1].to_owned(),
        content_type,
        body,
    })
}

fn rpc_body_limit(method: &str, target: &str) -> usize {
    match (method, target) {
        ("POST", "/v1/transaction") => MAX_TRANSACTION_BYTES,
        ("POST", "/v1/wallet/send" | "/v1/wallet/consolidate") => WALLET_JSON_BODY_LIMIT,
        ("POST", "/v1/block") => MAX_BLOCK_BYTES,
        ("POST", target) if target.starts_with("/v1/mine?") => 0,
        ("GET", _) => 0,
        _ => MAX_BLOCK_BYTES,
    }
}

fn write_rpc_response(stream: &mut impl Write, response: RpcResponse) -> Result<(), NodeError> {
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\n\r\n",
        response.status,
        response.reason,
        response.content_type,
        response.body.len()
    );
    stream
        .write_all(header.as_bytes())
        .map_err(NodeError::RpcIo)?;
    stream.write_all(&response.body).map_err(NodeError::RpcIo)?;
    stream.flush().map_err(NodeError::RpcIo)
}

fn insecure_dev_destination(secret_byte: u8) -> [u8; 32] {
    let signing = SigningKey::from_bytes(&[secret_byte; 32])
        .expect("fixed nonzero development signing key must be valid");
    signing.verifying_key().to_bytes().into()
}

fn insecure_dev_wallet_signing_key() -> SigningKey {
    SigningKey::from_bytes(&[0x13; 32])
        .expect("fixed nonzero development wallet signing key must be valid")
}

fn io_error(operation: &'static str, path: impl AsRef<Path>, source: io::Error) -> NodeError {
    NodeError::Io {
        operation,
        path: path.as_ref().to_path_buf(),
        source,
    }
}

fn load_or_create_metadata(data_dir: &Path, fingerprint: [u8; 32]) -> Result<(), NodeError> {
    let path = data_dir.join(METADATA_FILE);
    match OpenOptions::new().read(true).open(&path) {
        Ok(mut file) => {
            if file
                .metadata()
                .map_err(|source| io_error("inspect network metadata", &path, source))?
                .len()
                != METADATA_BYTES as u64
            {
                return Err(NodeError::InvalidMetadata);
            }
            let mut bytes = [0_u8; METADATA_BYTES];
            file.read_exact(&mut bytes).map_err(|source| {
                if source.kind() == io::ErrorKind::UnexpectedEof {
                    NodeError::InvalidMetadata
                } else {
                    io_error("read network metadata", &path, source)
                }
            })?;
            if bytes[..4] != METADATA_MAGIC
                || u16::from_le_bytes([bytes[4], bytes[5]]) != METADATA_VERSION
                || bytes[6..8] != [0, 0]
            {
                return Err(NodeError::InvalidMetadata);
            }
            if bytes[8..40] != fingerprint {
                return Err(NodeError::FingerprintMismatch);
            }
            Ok(())
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            let log_path = data_dir.join(BLOCK_LOG_FILE);
            match fs::metadata(&log_path) {
                Ok(metadata) if metadata.len() > 0 => return Err(NodeError::MissingMetadata),
                Ok(_) => {}
                Err(source) if source.kind() == io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(io_error("inspect existing block log", &log_path, source));
                }
            }
            let mut bytes = Vec::with_capacity(METADATA_BYTES);
            bytes.extend_from_slice(&METADATA_MAGIC);
            bytes.extend_from_slice(&METADATA_VERSION.to_le_bytes());
            bytes.extend_from_slice(&0_u16.to_le_bytes());
            bytes.extend_from_slice(&fingerprint);
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|source| io_error("create network metadata", &path, source))?;
            file.write_all(&bytes)
                .map_err(|source| io_error("write network metadata", &path, source))?;
            file.sync_all()
                .map_err(|source| io_error("sync network metadata", &path, source))
        }
        Err(source) => Err(io_error("open network metadata", &path, source)),
    }
}

fn encode_record(accepted_at: u64, block: &[u8]) -> Result<Vec<u8>, NodeError> {
    let block_len = u32::try_from(block.len())
        .map_err(|_| NodeError::CorruptLog("block length exceeds u32".to_owned()))?;
    if block.len() > MAX_BLOCK_BYTES {
        return Err(NodeError::CorruptLog("block exceeds wire limit".to_owned()));
    }
    let mut record = Vec::with_capacity(RECORD_HEADER_BYTES + block.len() + RECORD_CHECKSUM_BYTES);
    record.extend_from_slice(&RECORD_MAGIC);
    record.extend_from_slice(&RECORD_VERSION.to_le_bytes());
    record.extend_from_slice(&0_u16.to_le_bytes());
    record.extend_from_slice(&accepted_at.to_le_bytes());
    record.extend_from_slice(&block_len.to_le_bytes());
    record.extend_from_slice(block);
    let checksum = record_checksum(&record);
    record.extend_from_slice(&checksum);
    Ok(record)
}

fn record_checksum(record_without_checksum: &[u8]) -> [u8; 32] {
    let mut hasher = Hasher::new_derive_key(RECORD_CHECKSUM_DOMAIN);
    hasher.update(record_without_checksum);
    *hasher.finalize().as_bytes()
}

fn replay_log(
    path: &Path,
    state: &mut ChainState,
    index: &mut BlockIndex,
    verifier: &ConsensusPowVerifier,
    params: NetworkParams,
    network_id: [u8; 32],
) -> Result<(), NodeError> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(io_error("open block log for replay", path, source)),
    };
    let mut reader = BufReader::new(file);
    let mut record_index = 0_u64;
    loop {
        let mut header = [0_u8; RECORD_HEADER_BYTES];
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(()),
            Ok(1) => {}
            Ok(_) => unreachable!("one-byte read cannot return more than one byte"),
            Err(source) => return Err(io_error("read block log", path, source)),
        }
        reader.read_exact(&mut header[1..]).map_err(|source| {
            log_read_error(
                path,
                source,
                format!("record {record_index} has a truncated header"),
            )
        })?;
        if header[..4] != RECORD_MAGIC
            || u16::from_le_bytes([header[4], header[5]]) != RECORD_VERSION
            || header[6..8] != [0, 0]
        {
            return Err(NodeError::CorruptLog(format!(
                "record {record_index} has an invalid header"
            )));
        }
        let accepted_at = u64::from_le_bytes(header[8..16].try_into().expect("fixed slice"));
        let block_len =
            u32::from_le_bytes(header[16..20].try_into().expect("fixed slice")) as usize;
        if block_len > MAX_BLOCK_BYTES {
            return Err(NodeError::CorruptLog(format!(
                "record {record_index} block exceeds the wire limit"
            )));
        }
        let mut block_bytes = vec![0_u8; block_len];
        reader.read_exact(&mut block_bytes).map_err(|source| {
            log_read_error(
                path,
                source,
                format!("record {record_index} has a truncated block"),
            )
        })?;
        let mut checksum = [0_u8; RECORD_CHECKSUM_BYTES];
        reader.read_exact(&mut checksum).map_err(|source| {
            log_read_error(
                path,
                source,
                format!("record {record_index} has a truncated checksum"),
            )
        })?;
        let mut checksummed = Vec::with_capacity(RECORD_HEADER_BYTES + block_len);
        checksummed.extend_from_slice(&header);
        checksummed.extend_from_slice(&block_bytes);
        if checksum != record_checksum(&checksummed) {
            return Err(NodeError::CorruptLog(format!(
                "record {record_index} checksum mismatch"
            )));
        }
        let block = decode_block(&block_bytes, network_id).map_err(|error| {
            NodeError::CorruptLog(format!("record {record_index} cannot decode: {error}"))
        })?;
        let reencoded = encode_block(&block).map_err(|error| {
            NodeError::CorruptLog(format!("record {record_index} cannot re-encode: {error}"))
        })?;
        if reencoded != block_bytes {
            return Err(NodeError::CorruptLog(format!(
                "record {record_index} is not canonical"
            )));
        }
        let prepared = prepare_block(
            state,
            index,
            params,
            verifier,
            &block,
            accepted_at,
            block_bytes,
        )
        .map_err(|error| {
            NodeError::CorruptLog(format!("record {record_index} fails fork replay: {error}"))
        })?;
        commit_prepared(state, index, prepared).map_err(|error| {
            NodeError::CorruptLog(format!(
                "record {record_index} cannot restore fork state: {error}"
            ))
        })?;
        record_index += 1;
    }
}

fn log_read_error(path: &Path, source: io::Error, truncated_message: String) -> NodeError {
    if source.kind() == io::ErrorKind::UnexpectedEof {
        NodeError::CorruptLog(truncated_message)
    } else {
        io_error("read block log", path, source)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Seek, SeekFrom};
    use std::sync::atomic::{AtomicU64, Ordering};

    use cmfd_consensus::{
        ForgeMatrixV2CompactProof, InputWitness, TEST_PROFILE, TRANSACTION_VERSION, TxInput,
        TxOutput,
    };

    use super::*;

    static NEXT_TEST_DIR: AtomicU64 = AtomicU64::new(0);

    fn test_dir(name: &str) -> PathBuf {
        let id = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("cmfd-node-{name}-{}-{id}", std::process::id()))
    }

    fn clean_test_dir(path: &Path) {
        if path.exists() {
            fs::remove_dir_all(path).expect("remove isolated test directory");
        }
    }

    fn mined_candidate(node: &Node, now: u64) -> Block {
        let template = node
            .build_template(default_miner_destination(), now)
            .unwrap();
        let proof = node.verifier.mine(&template.challenge, 0, 100).unwrap();
        Block {
            version: BLOCK_VERSION,
            challenge: template.challenge,
            proof,
            coinbase: template.coinbase,
            transactions: Vec::new(),
        }
    }

    fn mined_child(node: &Node, parent: [u8; 32], timestamp: u64, miner_seed: u8) -> Block {
        mined_child_with_transactions(node, parent, timestamp, miner_seed, Vec::new())
    }

    fn mined_child_with_transactions(
        node: &Node,
        parent: [u8; 32],
        timestamp: u64,
        miner_seed: u8,
        transactions: Vec<Transaction>,
    ) -> Block {
        let state = rebuild_state_to(&node.index, node.params, &node.verifier, parent).unwrap();
        let height = state.next_height();
        let destination = insecure_dev_destination(miner_seed);
        let fees = state
            .validate_transactions_for_next_block(&transactions)
            .unwrap()
            .total_burned_fees;
        let allocation = node
            .params
            .monetary_policy
            .allocation(height, fees)
            .unwrap();
        let coinbase = Coinbase::new(height, allocation, destination, node.params.rewards);
        let mut commitments = vec![coinbase.commitment(node.params.network_id)];
        commitments.extend(transactions.iter().map(Transaction::txid));
        let challenge = BlockChallenge {
            network_id: node.params.network_id,
            previous_block: parent,
            transaction_root: merkle_root(&commitments),
            height,
            timestamp,
            target: state.expected_target().unwrap(),
        };
        let proof = node
            .verifier
            .mine(&challenge, 0, DEFAULT_MINING_ATTEMPTS)
            .unwrap();
        Block {
            version: BLOCK_VERSION,
            challenge,
            proof,
            coinbase,
            transactions,
        }
    }

    fn spend_coinbase_output(
        node: &Node,
        block: &Block,
        output_index: u32,
        owner_secret: u8,
        recipient_secret: u8,
        fee: u64,
    ) -> Transaction {
        let previous_output = &block.coinbase.outputs[output_index as usize];
        let owner = SigningKey::from_bytes(&[owner_secret; 32]).unwrap();
        let mut transaction = Transaction {
            network_id: node.params.network_id,
            version: TRANSACTION_VERSION,
            inputs: vec![TxInput {
                previous: OutPoint {
                    txid: block.coinbase_outpoint_id(),
                    index: output_index,
                },
                witness: InputWitness::Key {
                    public_key: [0; 32],
                    signature: Vec::new(),
                },
            }],
            outputs: vec![TxOutput {
                value: previous_output.value.checked_sub(fee).unwrap(),
                lock: OutputLock::Key(insecure_dev_destination(recipient_secret)),
                spendable_height: node.state.next_height(),
            }],
        };
        transaction.sign_all(&[&owner]).unwrap();
        transaction
    }

    #[test]
    fn mempool_policy_rejects_zero_fee_unconfirmed_duplicate_and_conflicting_spends_atomically() {
        let path = test_dir("mempool-policy");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let now = DEVNET_GENESIS_TIMESTAMP + 60;
        let funding = node
            .mine_once(default_miner_destination(), now, DEFAULT_MINING_ATTEMPTS)
            .unwrap();

        let zero_fee = spend_coinbase_output(&node, &funding, 2, 0x12, 0x31, 0);
        assert!(matches!(
            node.submit_transaction(zero_fee),
            Err(NodeError::MempoolFeeTooLow {
                required: 1,
                actual: 0
            })
        ));
        assert!(node.mempool.is_empty());

        let transaction = spend_coinbase_output(&node, &funding, 2, 0x12, 0x31, 10);
        let recipient = SigningKey::from_bytes(&[0x31; 32]).unwrap();
        let mut unconfirmed_child = Transaction {
            network_id: node.params.network_id,
            version: TRANSACTION_VERSION,
            inputs: vec![TxInput {
                previous: OutPoint {
                    txid: transaction.txid(),
                    index: 0,
                },
                witness: InputWitness::Key {
                    public_key: [0; 32],
                    signature: Vec::new(),
                },
            }],
            outputs: vec![TxOutput {
                value: transaction.outputs[0].value - 1,
                lock: OutputLock::Key(insecure_dev_destination(0x32)),
                spendable_height: node.state.next_height(),
            }],
        };
        unconfirmed_child.sign_all(&[&recipient]).unwrap();
        let missing = unconfirmed_child.inputs[0].previous;
        assert!(matches!(
            node.submit_transaction(unconfirmed_child),
            Err(NodeError::MempoolUnconfirmedInput(outpoint)) if outpoint == missing
        ));

        let entry = node.submit_transaction(transaction.clone()).unwrap();
        let original_bytes = node.mempool_bytes;
        assert_eq!(entry.fee_burned, 10);
        assert!(matches!(
            node.submit_transaction(transaction.clone()),
            Err(NodeError::DuplicateMempoolTransaction(txid)) if txid == transaction.txid()
        ));

        let conflict = spend_coinbase_output(&node, &funding, 2, 0x12, 0x33, 11);
        let previous = conflict.inputs[0].previous;
        assert!(matches!(
            node.submit_transaction(conflict),
            Err(NodeError::MempoolInputConflict(outpoint)) if outpoint == previous
        ));
        assert_eq!(node.mempool.len(), 1);
        assert_eq!(node.mempool_bytes, original_bytes);
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn mempool_fee_policy_rounds_canonical_bytes_up_to_the_next_kib() {
        let path = test_dir("mempool-fee-size");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let funding = node
            .mine_once(
                default_miner_destination(),
                DEVNET_GENESIS_TIMESTAMP + 60,
                DEFAULT_MINING_ATTEMPTS,
            )
            .unwrap();
        let mut transaction = spend_coinbase_output(&node, &funding, 2, 0x12, 0x34, 1);
        let total = transaction.outputs[0].value;
        transaction.outputs = (0_u8..64)
            .map(|index| TxOutput {
                value: if index == 0 { total - 63 } else { 1 },
                lock: OutputLock::Key(insecure_dev_destination(0x40 + index)),
                spendable_height: node.state.next_height(),
            })
            .collect();
        let owner = SigningKey::from_bytes(&[0x12; 32]).unwrap();
        transaction.sign_all(&[&owner]).unwrap();
        let encoded_bytes = encode_transaction(&transaction).unwrap().len();
        let required = u64::try_from(encoded_bytes.div_ceil(1024)).unwrap();
        assert!(required > 1);
        assert!(matches!(
            node.submit_transaction(transaction),
            Err(NodeError::MempoolFeeTooLow {
                required: actual_required,
                actual: 1
            }) if actual_required == required
        ));
        assert!(node.mempool.is_empty());
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn complete_ordered_pool_validation_is_atomic() {
        let path = test_dir("mempool-aggregate-atomic");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let funding = node
            .mine_once(
                default_miner_destination(),
                DEVNET_GENESIS_TIMESTAMP + 60,
                DEFAULT_MINING_ATTEMPTS,
            )
            .unwrap();
        let channel_id = [0x5a; 32];
        let mut first = spend_coinbase_output(&node, &funding, 1, 0x11, 0x35, 1);
        first.outputs[0].lock = OutputLock::InferenceChannel { channel_id };
        first
            .sign_all(&[&SigningKey::from_bytes(&[0x11; 32]).unwrap()])
            .unwrap();
        let mut second = spend_coinbase_output(&node, &funding, 2, 0x12, 0x36, 1);
        second.outputs[0].lock = OutputLock::InferenceChannel { channel_id };
        second
            .sign_all(&[&SigningKey::from_bytes(&[0x12; 32]).unwrap()])
            .unwrap();

        node.submit_transaction(first.clone()).unwrap();
        let bytes_before = node.mempool_bytes;
        assert!(matches!(
            node.submit_transaction(second),
            Err(NodeError::Chain(ChainError::DuplicateChannel))
        ));
        assert_eq!(node.mempool.len(), 1);
        assert!(node.mempool.contains_key(&first.txid()));
        assert_eq!(node.mempool_bytes, bytes_before);
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn mempool_count_and_byte_caps_reject_without_mutation() {
        let path = test_dir("mempool-caps");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let funding = node
            .mine_once(
                default_miner_destination(),
                DEVNET_GENESIS_TIMESTAMP + 60,
                DEFAULT_MINING_ATTEMPTS,
            )
            .unwrap();
        let transaction = spend_coinbase_output(&node, &funding, 2, 0x12, 0x37, 1);
        let encoded_bytes = encode_transaction(&transaction).unwrap().len();
        for index in 0..MAX_MEMPOOL_TRANSACTIONS {
            let mut txid = [0_u8; 32];
            txid[..8].copy_from_slice(&(index as u64).to_be_bytes());
            assert_ne!(txid, transaction.txid());
            node.mempool.insert(
                txid,
                MempoolEntry {
                    txid,
                    transaction: transaction.clone(),
                    encoded_bytes,
                    fee_burned: 1,
                },
            );
        }
        assert!(matches!(
            node.submit_transaction(transaction.clone()),
            Err(NodeError::MempoolTransactionLimit)
        ));
        assert_eq!(node.mempool.len(), MAX_MEMPOOL_TRANSACTIONS);

        node.mempool.clear();
        node.mempool_bytes = MAX_MEMPOOL_BYTES;
        assert!(matches!(
            node.submit_transaction(transaction),
            Err(NodeError::MempoolByteLimit)
        ));
        assert!(node.mempool.is_empty());
        assert_eq!(node.mempool_bytes, MAX_MEMPOOL_BYTES);
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn template_orders_transactions_burns_fees_and_mining_clears_confirmed_entries() {
        let path = test_dir("mempool-template");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let now = DEVNET_GENESIS_TIMESTAMP + 60;
        let funding = node
            .mine_once(default_miner_destination(), now, DEFAULT_MINING_ATTEMPTS)
            .unwrap();
        let first = spend_coinbase_output(&node, &funding, 1, 0x11, 0x38, 1_000);
        let second = spend_coinbase_output(&node, &funding, 2, 0x12, 0x39, 2_000);
        if first.txid() < second.txid() {
            node.submit_transaction(second.clone()).unwrap();
            node.submit_transaction(first.clone()).unwrap();
        } else {
            node.submit_transaction(first.clone()).unwrap();
            node.submit_transaction(second.clone()).unwrap();
        }

        let template = node
            .build_template(default_miner_destination(), now + 60)
            .unwrap();
        let mut expected = vec![first, second];
        expected.sort_by_key(Transaction::txid);
        assert_eq!(template.transactions, expected);
        assert_eq!(template.total_fees_burned, 3_000);
        let allocation = node
            .params
            .monetary_policy
            .allocation(node.state.next_height(), 3_000)
            .unwrap();
        assert_eq!(
            template.coinbase,
            Coinbase::new(
                node.state.next_height(),
                allocation,
                default_miner_destination(),
                node.params.rewards,
            )
        );
        let mut commitments = vec![template.coinbase.commitment(node.params.network_id)];
        commitments.extend(expected.iter().map(Transaction::txid));
        assert_eq!(
            template.challenge.transaction_root,
            merkle_root(&commitments)
        );

        let block = node
            .mine_once(
                default_miner_destination(),
                now + 60,
                DEFAULT_MINING_ATTEMPTS,
            )
            .unwrap();
        assert_eq!(block.transactions, expected);
        let status = node.status().unwrap();
        assert_eq!(status.mempool_transactions, 0);
        assert_eq!(status.mempool_bytes, 0);
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn side_branch_leaves_mempool_untouched_but_activating_reorg_evicts_conflict() {
        let path = test_dir("mempool-reorg");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let genesis = node.params.genesis_hash;
        let t1 = DEVNET_GENESIS_TIMESTAMP + 60;
        let t2 = t1 + 60;
        let t3 = t2 + 60;
        let common = mined_child(&node, genesis, t1, 0x70);
        node.submit_block(common.clone(), t1).unwrap();
        let active = mined_child(&node, common.block_id(), t2, 0x71);
        node.submit_block(active.clone(), t2).unwrap();

        let pooled = spend_coinbase_output(&node, &common, 2, 0x12, 0x72, 10);
        node.submit_transaction(pooled.clone()).unwrap();
        let pool_bytes = node.mempool_bytes;
        let conflicting = spend_coinbase_output(&node, &common, 2, 0x12, 0x73, 11);
        let side =
            mined_child_with_transactions(&node, common.block_id(), t2, 0x74, vec![conflicting]);
        node.submit_block(side.clone(), t2).unwrap();
        assert_eq!(node.state.tip(), active.block_id());
        assert_eq!(node.mempool.len(), 1);
        assert!(node.mempool.contains_key(&pooled.txid()));
        assert_eq!(node.mempool_bytes, pool_bytes);

        let heavier = mined_child(&node, side.block_id(), t3, 0x75);
        node.submit_block(heavier.clone(), t3).unwrap();
        assert_eq!(node.state.tip(), heavier.block_id());
        assert!(node.mempool.is_empty());
        assert_eq!(node.mempool_bytes, 0);
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn devnet_params_bind_the_exact_v2_verifier() {
        let params = devnet_params().unwrap();
        assert_eq!(params.network_id, DEVNET_NETWORK_ID);
        assert!(matches!(params.pow, PowParameters::V2Reference(_)));

        let reference = v2_test_reference().unwrap();
        let verifier = ConsensusPowVerifier::v2_reference(reference);
        assert_eq!(verifier.parameters(), params.pow);
        assert!(ChainState::new(params, verifier).is_ok());

        let legacy = ConsensusPowVerifier::v1_legacy(TEST_PROFILE).unwrap();
        assert!(matches!(
            ChainState::new(params, legacy),
            Err(ChainError::PowParameterMismatch)
        ));
    }

    #[test]
    fn peer_hello_binds_live_tip_and_canonical_u512_work() {
        let path = test_dir("peer-hello");
        clean_test_dir(&path);
        let node = Node::open(&path).unwrap();
        let hello = node.peer_hello();
        assert_eq!(hello.network_id, node.params.network_id);
        assert_eq!(hello.consensus_fingerprint, node.fingerprint);
        assert_eq!(hello.tip, node.state.tip());
        assert_eq!(hello.height, 0);
        assert_eq!(hello.cumulative_work, peer::ChainWork::ZERO);
        assert_ne!(hello.node_nonce, [0; 32]);
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn data_directory_lock_rejects_a_concurrent_node_open() {
        let path = test_dir("lock-concurrent");
        clean_test_dir(&path);
        let node = Node::open(&path).unwrap();
        let error = match Node::open(&path) {
            Ok(_) => panic!("a second node opened a locked data directory"),
            Err(error) => error,
        };
        assert!(matches!(&error, NodeError::DataDirLocked(_)));
        assert!(error.to_string().contains("locked by another running node"));
        assert!(path.join(LOCK_FILE).exists());
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn dropping_a_node_releases_the_persistent_data_directory_lock() {
        let path = test_dir("lock-drop");
        clean_test_dir(&path);
        let node = Node::open(&path).unwrap();
        drop(node);

        assert!(path.join(LOCK_FILE).exists());
        drop(Node::open(&path).unwrap());
        assert!(path.join(LOCK_FILE).exists());
        clean_test_dir(&path);
    }

    #[test]
    fn preexisting_unlocked_lock_file_is_refreshed_without_blocking_open() {
        let path = test_dir("lock-preexisting");
        clean_test_dir(&path);
        fs::create_dir_all(&path).unwrap();
        let lock_path = path.join(LOCK_FILE);
        fs::write(&lock_path, b"stale diagnostic contents\n").unwrap();

        drop(Node::open(&path).unwrap());

        assert_eq!(
            fs::read_to_string(&lock_path).unwrap(),
            format!("pid={}\n", std::process::id())
        );
        clean_test_dir(&path);
    }

    #[test]
    fn mine_restart_and_strict_replay_restore_the_tip() {
        let path = test_dir("replay");
        clean_test_dir(&path);
        let (block_id, fingerprint) = {
            let mut node = Node::open(&path).unwrap();
            let block = node
                .mine_once(default_miner_destination(), 1_800_000_000, 100)
                .unwrap();
            assert!(matches!(block.proof, BlockProof::V2Reference(_)));
            assert_eq!(node.status().unwrap().accepted_height, 1);
            (block.block_id(), node.fingerprint)
        };

        let node = Node::open(&path).unwrap();
        let status = node.status().unwrap();
        assert_eq!(status.accepted_height, 1);
        assert_eq!(status.next_height, 2);
        assert_eq!(status.tip, hex::encode(block_id));
        assert_eq!(status.consensus_fingerprint, hex::encode(fingerprint));
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn strictly_heavier_fork_reorgs_while_equal_work_keeps_the_tip() {
        let path = test_dir("heavier-fork");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let genesis = node.params.genesis_hash;
        let t1 = DEVNET_GENESIS_TIMESTAMP + 60;
        let t2 = t1 + 60;
        let t3 = t2 + 60;

        let a1 = mined_child(&node, genesis, t1, 0x21);
        node.submit_block(a1.clone(), t1).unwrap();
        let a2 = mined_child(&node, a1.block_id(), t2, 0x22);
        node.submit_block(a2.clone(), t2).unwrap();
        let active_work = node.cumulative_work();

        let b1 = mined_child(&node, genesis, t1, 0x31);
        node.submit_block(b1.clone(), t1).unwrap();
        assert_eq!(node.state.tip(), a2.block_id());
        let b2 = mined_child(&node, b1.block_id(), t2, 0x32);
        node.submit_block(b2.clone(), t2).unwrap();

        assert_eq!(
            node.index.blocks[&b2.block_id()].cumulative_work,
            node.index.active_work
        );
        assert_eq!(node.cumulative_work(), active_work);
        assert_eq!(node.state.tip(), a2.block_id());

        let b3 = mined_child(&node, b2.block_id(), t3, 0x33);
        node.submit_block(b3.clone(), t3).unwrap();
        assert_eq!(node.state.tip(), b3.block_id());
        assert_eq!(node.state.next_height(), 4);
        assert_eq!(
            node.index.active_chain,
            vec![genesis, b1.block_id(), b2.block_id(), b3.block_id()]
        );
        assert!(node.index.active_work > node.index.blocks[&a2.block_id()].cumulative_work);
        assert_eq!(node.status().unwrap().cumulative_work.len(), 128);
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn invalid_side_block_is_neither_indexed_nor_persisted() {
        let path = test_dir("invalid-side");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let genesis = node.params.genesis_hash;
        let t1 = DEVNET_GENESIS_TIMESTAMP + 60;
        let t2 = t1 + 60;

        let active = mined_child(&node, genesis, t1, 0x41);
        node.submit_block(active.clone(), t1).unwrap();
        let active_2 = mined_child(&node, active.block_id(), t2, 0x42);
        let active_tip = active_2.block_id();
        node.submit_block(active_2, t2).unwrap();
        let side = mined_child(&node, genesis, t1, 0x51);
        node.submit_block(side.clone(), t1).unwrap();

        let mut invalid = mined_child(&node, side.block_id(), t2, 0x52);
        invalid.challenge.target[31] ^= 1;
        let invalid_id = invalid.block_id();
        let log_len = node.log.metadata().unwrap().len();
        assert!(matches!(
            node.submit_block(invalid, t2),
            Err(NodeError::Chain(ChainError::UnexpectedTarget))
        ));
        assert!(!node.contains_block(invalid_id));
        assert_eq!(node.log.metadata().unwrap().len(), log_len);
        assert_eq!(node.state.tip(), active_tip);
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn restart_reconstructs_side_branches_active_tip_and_work() {
        let path = test_dir("fork-restart");
        clean_test_dir(&path);
        let (tip, work, side_id, block_count) = {
            let mut node = Node::open(&path).unwrap();
            let genesis = node.params.genesis_hash;
            let t1 = DEVNET_GENESIS_TIMESTAMP + 60;
            let t2 = t1 + 60;
            let t3 = t2 + 60;
            let a1 = mined_child(&node, genesis, t1, 0x61);
            node.submit_block(a1.clone(), t1).unwrap();
            let a2 = mined_child(&node, a1.block_id(), t2, 0x62);
            node.submit_block(a2.clone(), t2).unwrap();
            let b1 = mined_child(&node, genesis, t1, 0x71);
            node.submit_block(b1.clone(), t1).unwrap();
            let b2 = mined_child(&node, b1.block_id(), t2, 0x72);
            node.submit_block(b2.clone(), t2).unwrap();
            let b3 = mined_child(&node, b2.block_id(), t3, 0x73);
            node.submit_block(b3.clone(), t3).unwrap();
            (
                b3.block_id(),
                node.cumulative_work(),
                a2.block_id(),
                node.index.blocks.len(),
            )
        };

        let node = Node::open(&path).unwrap();
        assert_eq!(node.state.tip(), tip);
        assert_eq!(node.cumulative_work(), work);
        assert_eq!(node.index.blocks.len(), block_count);
        assert!(node.contains_block(side_id));
        assert!(node.canonical_block(side_id).is_some());
        assert_eq!(node.status().unwrap().cumulative_work, hex::encode(work.0));
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn locator_and_inventory_are_bounded_and_follow_active_order() {
        let path = test_dir("locator");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let genesis = node.params.genesis_hash;
        let mut ids = Vec::new();
        let mut parent = genesis;
        for offset in 1_u64..=14 {
            let timestamp = DEVNET_GENESIS_TIMESTAMP + offset * 60;
            let block = mined_child(&node, parent, timestamp, 0x80 + offset as u8);
            parent = block.block_id();
            node.submit_block(block, timestamp).unwrap();
            ids.push(parent);
        }

        assert!(node.block_locator(0).is_empty());
        assert_eq!(node.block_locator(1), vec![genesis]);
        let locator = node.block_locator(6);
        assert!(locator.len() <= 6);
        assert_eq!(locator.first(), ids.last());
        assert_eq!(locator.last(), Some(&genesis));
        assert!(locator.windows(2).all(|pair| pair[0] != pair[1]));

        assert_eq!(
            node.inventory_after(&[ids[3]], [0; 32], 3),
            ids[4..7].to_vec()
        );
        assert_eq!(
            node.inventory_after(&[ids[3]], ids[5], 10),
            ids[4..=5].to_vec()
        );
        assert!(node.inventory_after(&[[0xaa; 32]], [0; 32], 10).is_empty());
        assert!(node.inventory_after(&[genesis], [0; 32], 0).is_empty());
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn active_extension_is_not_committed_when_durable_append_fails() {
        let path = test_dir("durable-before-commit");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let timestamp = DEVNET_GENESIS_TIMESTAMP + 60;
        let block = mined_child(&node, node.params.genesis_hash, timestamp, 0xa1);
        let block_id = block.block_id();
        let tip = node.state.tip();
        let work = node.cumulative_work();
        let log_path = path.join(BLOCK_LOG_FILE);
        let log_len = node.log.metadata().unwrap().len();
        let read_only = OpenOptions::new().read(true).open(&log_path).unwrap();
        drop(std::mem::replace(&mut node.log, read_only));

        assert!(matches!(
            node.submit_block(block, timestamp),
            Err(NodeError::Io {
                operation: "append block record",
                ..
            })
        ));
        assert_eq!(node.state.tip(), tip);
        assert_eq!(node.cumulative_work(), work);
        assert!(!node.contains_block(block_id));
        assert_eq!(fs::metadata(log_path).unwrap().len(), log_len);
        assert!(node.storage_faulted);
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn duplicate_and_unknown_parent_have_stable_errors() {
        let path = test_dir("block-identity-errors");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let timestamp = DEVNET_GENESIS_TIMESTAMP + 60;
        let block = mined_child(&node, node.params.genesis_hash, timestamp, 0xb1);
        let block_id = block.block_id();
        node.submit_block(block.clone(), timestamp).unwrap();
        assert!(matches!(
            node.submit_block(block, timestamp),
            Err(NodeError::DuplicateBlock(id)) if id == block_id
        ));

        let mut orphan = mined_child(&node, block_id, timestamp + 60, 0xb2);
        orphan.challenge.previous_block = [0xcc; 32];
        assert!(matches!(
            node.submit_block(orphan, timestamp + 60),
            Err(NodeError::UnknownParent(parent)) if parent == [0xcc; 32]
        ));
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn truncated_log_is_refused_instead_of_recovered_silently() {
        let path = test_dir("truncated");
        clean_test_dir(&path);
        {
            let mut node = Node::open(&path).unwrap();
            node.mine_once(default_miner_destination(), 1_800_000_000, 100)
                .unwrap();
        }
        let log_path = path.join(BLOCK_LOG_FILE);
        let file = OpenOptions::new().write(true).open(&log_path).unwrap();
        let len = file.metadata().unwrap().len();
        file.set_len(len - 1).unwrap();
        drop(file);

        assert!(matches!(Node::open(&path), Err(NodeError::CorruptLog(_))));
        clean_test_dir(&path);
    }

    #[test]
    fn checksum_corruption_is_refused() {
        let path = test_dir("checksum");
        clean_test_dir(&path);
        {
            let mut node = Node::open(&path).unwrap();
            node.mine_once(default_miner_destination(), 1_800_000_000, 100)
                .unwrap();
        }
        let log_path = path.join(BLOCK_LOG_FILE);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&log_path)
            .unwrap();
        let len = file.metadata().unwrap().len();
        file.seek(SeekFrom::Start(len - 1)).unwrap();
        let mut byte = [0_u8; 1];
        file.read_exact(&mut byte).unwrap();
        file.seek(SeekFrom::Start(len - 1)).unwrap();
        file.write_all(&[byte[0] ^ 1]).unwrap();
        file.sync_all().unwrap();
        drop(file);

        assert!(matches!(Node::open(&path), Err(NodeError::CorruptLog(_))));
        clean_test_dir(&path);
    }

    #[test]
    fn wrong_fingerprint_is_refused() {
        let path = test_dir("fingerprint");
        clean_test_dir(&path);
        drop(Node::open(&path).unwrap());
        let metadata_path = path.join(METADATA_FILE);
        let mut metadata = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&metadata_path)
            .unwrap();
        metadata.seek(SeekFrom::Start(8)).unwrap();
        metadata.write_all(&[0; 32]).unwrap();
        metadata.sync_all().unwrap();
        drop(metadata);

        assert!(matches!(
            Node::open(&path),
            Err(NodeError::FingerprintMismatch)
        ));
        clean_test_dir(&path);
    }

    #[test]
    fn missing_metadata_cannot_rebind_an_existing_log() {
        let path = test_dir("missing-metadata");
        clean_test_dir(&path);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join(BLOCK_LOG_FILE), b"existing-chain-data").unwrap();

        assert!(matches!(Node::open(&path), Err(NodeError::MissingMetadata)));
        clean_test_dir(&path);
    }

    #[test]
    fn failed_validation_changes_neither_disk_nor_live_state() {
        let path = test_dir("atomic-validation");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let mut block = mined_candidate(&node, 1_800_000_000);
        block.challenge.previous_block = [0xaa; 32];
        let log_len = node.log.metadata().unwrap().len();

        assert!(matches!(
            node.submit_block(block, 1_800_000_000),
            Err(NodeError::UnknownParent(parent)) if parent == [0xaa; 32]
        ));
        assert_eq!(node.state.next_height(), 1);
        assert_eq!(node.log.metadata().unwrap().len(), log_len);
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn a_v2_proof_has_the_compact_devnet_shape() {
        let path = test_dir("v2-shape");
        clean_test_dir(&path);
        let node = Node::open(&path).unwrap();
        let block = mined_candidate(&node, 1_800_000_000);
        let BlockProof::V2Reference(ForgeMatrixV2CompactProof { proof_version, .. }) = block.proof
        else {
            panic!("Devnet-0 must not produce a v1 proof");
        };
        assert_eq!(proof_version, 1);
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn rpc_parser_accepts_a_bounded_binary_post() {
        let request = b"POST /v1/block HTTP/1.1\r\nContent-Type: application/octet-stream\r\nContent-Length: 3\r\n\r\nabc";
        let parsed = read_rpc_request(&mut &request[..]).unwrap();
        assert_eq!(parsed.method, "POST");
        assert_eq!(parsed.target, "/v1/block");
        assert_eq!(parsed.body, b"abc");
    }

    #[test]
    fn rpc_parser_rejects_oversized_headers_and_bodies() {
        let oversized_header = format!(
            "GET /health HTTP/1.1\r\nX-Padding: {}\r\n\r\n",
            "a".repeat(RPC_HEADER_LIMIT)
        );
        assert!(matches!(
            read_rpc_request(&mut oversized_header.as_bytes()),
            Err(NodeError::InvalidRpcRequest(_))
        ));

        let oversized_body = format!(
            "POST /v1/block HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BLOCK_BYTES + 1
        );
        assert!(matches!(
            read_rpc_request(&mut oversized_body.as_bytes()),
            Err(NodeError::InvalidRpcRequest(_))
        ));

        let oversized_transaction = format!(
            "POST /v1/transaction HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_TRANSACTION_BYTES + 1
        );
        assert!(matches!(
            read_rpc_request(&mut oversized_transaction.as_bytes()),
            Err(NodeError::InvalidRpcRequest(_))
        ));

        for endpoint in ["/v1/wallet/send", "/v1/wallet/consolidate"] {
            let oversized_wallet_request = format!(
                "POST {endpoint} HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
                WALLET_JSON_BODY_LIMIT + 1
            );
            assert!(matches!(
                read_rpc_request(&mut oversized_wallet_request.as_bytes()),
                Err(NodeError::InvalidRpcRequest(_))
            ));
        }

        let mine_with_body = format!(
            "POST /v1/mine?miner={}&attempts=1 HTTP/1.1\r\nContent-Length: 1\r\n\r\nx",
            hex::encode(default_miner_destination())
        );
        assert!(matches!(
            read_rpc_request(&mut mine_with_body.as_bytes()),
            Err(NodeError::InvalidRpcRequest(_))
        ));
    }

    #[test]
    fn mine_query_is_strict_and_attempts_are_capped() {
        let miner = hex::encode(default_miner_destination());
        assert_eq!(
            parse_mine_request(&format!(
                "/v1/mine?attempts={DEFAULT_MINING_ATTEMPTS}&miner={miner}"
            ))
            .unwrap(),
            (default_miner_destination(), DEFAULT_MINING_ATTEMPTS)
        );
        for target in [
            format!("/v1/mine?miner={miner}"),
            format!("/v1/mine?miner={miner}&attempts=0"),
            format!(
                "/v1/mine?miner={miner}&attempts={}",
                DEFAULT_MINING_ATTEMPTS + 1
            ),
            format!("/v1/mine?miner={miner}&miner={miner}&attempts=1"),
            format!("/v1/mine?miner={miner}&attempts=1&extra=1"),
        ] {
            assert!(matches!(
                parse_mine_request(&target),
                Err(NodeError::InvalidRpcRequest(_))
            ));
        }
    }

    #[test]
    fn rpc_parser_rejects_truncation_and_transfer_encoding() {
        let truncated = b"POST /v1/block HTTP/1.1\r\nContent-Length: 4\r\n\r\nabc";
        assert!(matches!(
            read_rpc_request(&mut &truncated[..]),
            Err(NodeError::InvalidRpcRequest(_))
        ));

        let chunked = b"POST /v1/block HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert!(matches!(
            read_rpc_request(&mut &chunked[..]),
            Err(NodeError::InvalidRpcRequest(_))
        ));
    }

    #[test]
    fn health_route_reports_a_storage_fault() {
        let path = test_dir("health");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let request = || RpcRequest {
            method: "GET".to_owned(),
            target: "/health".to_owned(),
            content_type: None,
            body: Vec::new(),
        };

        let healthy = route_rpc_request(request(), &mut node);
        assert_eq!(healthy.status, 200);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&healthy.body).unwrap()["ok"],
            true
        );

        node.storage_faulted = true;
        let faulted = route_rpc_request(request(), &mut node);
        assert_eq!(faulted.status, 503);
        let body: serde_json::Value = serde_json::from_slice(&faulted.body).unwrap();
        assert_eq!(body["ok"], false);
        assert_eq!(body["storage_healthy"], false);
        assert!(!node.status().unwrap().storage_healthy);
        let template = route_rpc_request(
            RpcRequest {
                method: "GET".to_owned(),
                target: format!(
                    "/v1/template?miner={}",
                    hex::encode(default_miner_destination())
                ),
                content_type: None,
                body: Vec::new(),
            },
            &mut node,
        );
        assert_eq!(template.status, 503);
        assert!(matches!(
            node.build_template(default_miner_destination(), 1_800_000_000),
            Err(NodeError::StorageFaulted)
        ));
        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn rpc_reader_enforces_an_absolute_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        let mut reader = DeadlineReader::new(&mut server, Duration::ZERO);
        let error = read_rpc_request(&mut reader).unwrap_err();
        assert!(matches!(
            error,
            NodeError::RpcIo(ref source) if source.kind() == io::ErrorKind::TimedOut
        ));
        drop(client);
    }

    #[test]
    fn block_route_decodes_validates_persists_and_applies() {
        let path = test_dir("block-route");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let now = unix_time_seconds().unwrap();
        let template = node
            .build_template(default_miner_destination(), now)
            .unwrap();
        let proof = node
            .verifier
            .mine(&template.challenge, 0, DEFAULT_MINING_ATTEMPTS)
            .unwrap();
        let block = Block {
            version: BLOCK_VERSION,
            challenge: template.challenge,
            proof,
            coinbase: template.coinbase,
            transactions: Vec::new(),
        };
        let request = RpcRequest {
            method: "POST".to_owned(),
            target: "/v1/block".to_owned(),
            content_type: Some("application/octet-stream".to_owned()),
            body: encode_block(&block).unwrap(),
        };

        let response = route_rpc_request(request, &mut node);
        assert_eq!(response.status, 200);
        assert_eq!(node.state.next_height(), 2);
        assert!(node.log.metadata().unwrap().len() > 0);
        drop(node);

        let replayed = Node::open(&path).unwrap();
        assert_eq!(replayed.status().unwrap().accepted_height, 1);
        drop(replayed);
        clean_test_dir(&path);
    }

    #[test]
    fn transaction_mempool_and_dev_mine_routes_form_a_loopback_mining_flow() {
        let path = test_dir("mempool-routes");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let now = unix_time_seconds().unwrap();
        let funding = node
            .mine_once(default_miner_destination(), now, DEFAULT_MINING_ATTEMPTS)
            .unwrap();
        let transaction = spend_coinbase_output(&node, &funding, 2, 0x12, 0x79, 10);
        let txid = transaction.txid();
        let encoded = encode_transaction(&transaction).unwrap();

        let submitted = route_rpc_request(
            RpcRequest {
                method: "POST".to_owned(),
                target: "/v1/transaction".to_owned(),
                content_type: Some("application/octet-stream; charset=binary".to_owned()),
                body: encoded,
            },
            &mut node,
        );
        assert_eq!(submitted.status, 200);
        let submitted_body: serde_json::Value = serde_json::from_slice(&submitted.body).unwrap();
        assert_eq!(submitted_body["txid"], hex::encode(txid));
        assert_eq!(submitted_body["fee_burned"], 10);

        let listed = route_rpc_request(
            RpcRequest {
                method: "GET".to_owned(),
                target: "/v1/mempool".to_owned(),
                content_type: None,
                body: Vec::new(),
            },
            &mut node,
        );
        assert_eq!(listed.status, 200);
        let listed_body: serde_json::Value = serde_json::from_slice(&listed.body).unwrap();
        assert_eq!(listed_body["transactions"], 1);
        assert_eq!(listed_body["entries"][0]["txid"], hex::encode(txid));
        assert_eq!(listed_body["entries"][0]["fee_burned"], 10);
        assert_eq!(listed_body["entries"][0]["fee_burned_atoms"], "10");
        assert!(listed_body["entries"][0]["fee_burned_atoms"].is_string());

        let mined = route_rpc_request(
            RpcRequest {
                method: "POST".to_owned(),
                target: format!(
                    "/v1/mine?miner={}&attempts={DEFAULT_MINING_ATTEMPTS}",
                    hex::encode(default_miner_destination())
                ),
                content_type: None,
                body: Vec::new(),
            },
            &mut node,
        );
        assert_eq!(mined.status, 200);
        let mined_body: serde_json::Value = serde_json::from_slice(&mined.body).unwrap();
        assert_eq!(mined_body["accepted"], true);
        assert_eq!(mined_body["height"], 2);
        assert_eq!(mined_body["block_id"].as_str().unwrap().len(), 64);
        assert!(node.mempool.is_empty());
        assert_eq!(node.mempool_bytes, 0);
        drop(node);
        clean_test_dir(&path);
    }

    fn mine_default_chain_to(node: &mut Node, height: u64) -> Vec<Block> {
        let mut blocks = Vec::new();
        while node.state.next_height() <= height {
            let block_height = node.state.next_height();
            blocks.push(
                node.mine_once(
                    default_miner_destination(),
                    DEVNET_GENESIS_TIMESTAMP + block_height * 60,
                    DEFAULT_MINING_ATTEMPTS,
                )
                .unwrap(),
            );
        }
        blocks
    }

    fn wallet_post(target: &str, value: serde_json::Value) -> RpcRequest {
        RpcRequest {
            method: "POST".to_owned(),
            target: target.to_owned(),
            content_type: Some("application/json; charset=utf-8".to_owned()),
            body: serde_json::to_vec(&value).unwrap(),
        }
    }

    fn synthetic_outpoint(order: u16) -> OutPoint {
        let mut txid = [0_u8; 32];
        txid[..2].copy_from_slice(&order.to_be_bytes());
        OutPoint { txid, index: 0 }
    }

    #[test]
    fn send_selection_finds_a_large_coin_beyond_the_old_128_outpoint_prefix() {
        let mut candidates: Vec<_> = (0..129)
            .map(|order| (synthetic_outpoint(order), 1_u64))
            .collect();
        let large = OutPoint {
            txid: [0xff; 32],
            index: 0,
        };
        candidates.push((large, 1_000));

        let mut old_outpoint_order = candidates.clone();
        old_outpoint_order.sort_unstable_by(|left, right| {
            left.0
                .txid
                .cmp(&right.0.txid)
                .then_with(|| left.0.index.cmp(&right.0.index))
        });
        let old_prefix_value: u64 = old_outpoint_order
            .iter()
            .take(MAX_TRANSACTION_INPUTS)
            .map(|(_, value)| value)
            .sum();
        assert_eq!(old_prefix_value, MAX_TRANSACTION_INPUTS as u64);
        assert!(old_prefix_value < 1_000);

        let selection = select_send_utxos(candidates.clone(), 1_000).unwrap();
        assert_eq!(selection.outpoints, vec![large]);
        assert_eq!(selection.selected_value, 1_000);
        assert_eq!(selection.available, 1_129);

        candidates.reverse();
        assert_eq!(select_send_utxos(candidates, 1_000).unwrap(), selection);
    }

    #[test]
    fn send_selection_uses_outpoint_ties_deterministically_and_caps_inputs() {
        let equal_value_candidates = vec![
            (synthetic_outpoint(3), 1_000),
            (synthetic_outpoint(1), 1_000),
            (synthetic_outpoint(2), 1_000),
        ];
        let selection = select_send_utxos(equal_value_candidates, 1_500).unwrap();
        assert_eq!(
            selection.outpoints,
            vec![synthetic_outpoint(1), synthetic_outpoint(2)]
        );
        assert_eq!(selection.selected_value, 2_000);

        let candidates: Vec<_> = (0..130)
            .rev()
            .map(|order| (synthetic_outpoint(order), 10_u64))
            .collect();
        let capped = select_send_utxos(candidates, 1_290).unwrap();
        assert_eq!(capped.outpoints.len(), MAX_TRANSACTION_INPUTS);
        assert_eq!(capped.selected_value, 1_280);
        assert_eq!(capped.available, 1_300);
        assert_eq!(capped.outpoints.first(), Some(&synthetic_outpoint(0)));
        assert_eq!(capped.outpoints.last(), Some(&synthetic_outpoint(127)));
    }

    #[test]
    fn wallet_starts_empty_and_serializes_only_string_atom_values() {
        let path = test_dir("wallet-empty");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();

        let snapshot = node.wallet_snapshot().unwrap();
        assert_eq!(snapshot.accepted_height, 0);
        assert_eq!(snapshot.next_height, 1);
        assert_eq!(snapshot.balances.spendable_atoms, "0");
        assert_eq!(snapshot.balances.immature_atoms, "0");
        assert_eq!(snapshot.balances.pending_atoms, "0");
        assert_eq!(snapshot.spendable_utxo_count, 0);
        assert_eq!(snapshot.immature_utxo_count, 0);
        assert_eq!(snapshot.reserved_utxo_count, 0);
        assert!(snapshot.history.is_empty());
        assert_eq!(
            snapshot.destination,
            hex::encode(default_miner_destination())
        );

        let response = route_rpc_request(
            RpcRequest {
                method: "GET".to_owned(),
                target: "/v1/wallet".to_owned(),
                content_type: None,
                body: Vec::new(),
            },
            &mut node,
        );
        assert_eq!(response.status, 200);
        let value: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        for field in ["spendable_atoms", "immature_atoms", "pending_atoms"] {
            assert!(value["balances"][field].is_string());
        }
        let serialized = String::from_utf8(response.body).unwrap();
        assert!(serialized.contains("INSECURE DEVNET-0 DEMO WALLET"));
        assert!(!serialized.contains(&hex::encode([0x13; 32])));

        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn wallet_coinbase_is_immature_until_next_height_101() {
        let path = test_dir("wallet-maturity");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let first = mine_default_chain_to(&mut node, 1).remove(0);
        let first_reward = first.coinbase.outputs[0].value;

        let early = node.wallet_snapshot().unwrap();
        assert_eq!(early.next_height, 2);
        assert_eq!(early.balances.spendable_atoms, "0");
        assert_eq!(early.balances.immature_atoms, first_reward.to_string());
        assert_eq!(early.immature_utxo_count, 1);
        assert_eq!(early.history[0].status, "immature");

        mine_default_chain_to(&mut node, 99);
        let through_100 = node.wallet_snapshot().unwrap();
        assert_eq!(through_100.next_height, 100);
        assert_eq!(through_100.balances.spendable_atoms, "0");
        assert_eq!(through_100.immature_utxo_count, 99);

        mine_default_chain_to(&mut node, 100);
        let mature = node.wallet_snapshot().unwrap();
        assert_eq!(mature.next_height, 101);
        assert_eq!(mature.balances.spendable_atoms, first_reward.to_string());
        assert_eq!(mature.spendable_utxo_count, 1);
        assert_eq!(mature.immature_utxo_count, 99);
        assert_eq!(mature.history.len(), MAX_WALLET_HISTORY);
        assert_eq!(mature.history[0].height, Some(100));

        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn wallet_send_reserves_burns_confirms_and_reconstructs_after_restart() {
        let path = test_dir("wallet-send-restart");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let first = mine_default_chain_to(&mut node, 100).remove(0);
        let input_atoms = first.coinbase.outputs[0].value;
        let recipient = insecure_dev_destination(0x79);
        let amount = COIN;
        let fee = 1_u64;

        let response = route_rpc_request(
            wallet_post(
                "/v1/wallet/send",
                json!({
                    "recipient": hex::encode(recipient),
                    "amount": "1.00000000",
                    "fee": "0.00000001",
                }),
            ),
            &mut node,
        );
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert!(body["amount_atoms"].is_string());
        assert!(body["fee_burned_atoms"].is_string());
        assert!(body["change_atoms"].is_string());
        assert_eq!(body["amount_atoms"], amount.to_string());
        assert_eq!(body["fee_burned_atoms"], fee.to_string());
        assert_eq!(
            body["change_atoms"],
            (input_atoms - amount - fee).to_string()
        );
        let entry = node.mempool.values().next().unwrap();
        assert_eq!(body["txid"], hex::encode(entry.txid));
        assert_eq!(entry.transaction.txid(), entry.txid);
        assert_eq!(entry.fee_burned, fee);

        let pending = node.wallet_snapshot().unwrap();
        assert_eq!(pending.spendable_utxo_count, 0);
        assert_eq!(pending.reserved_utxo_count, 1);
        assert_eq!(pending.balances.spendable_atoms, "0");
        assert_eq!(
            pending.balances.pending_atoms,
            (input_atoms - amount - fee).to_string()
        );
        assert_eq!(pending.history[0].kind, "sent");
        assert_eq!(pending.history[0].status, "pending");
        assert_eq!(
            pending.history[0].net_amount_atoms,
            format!("-{}", amount + fee)
        );
        assert_eq!(pending.history[0].fee_burned_atoms, fee.to_string());
        assert_eq!(
            pending.history[0].counterparty,
            Some(hex::encode(recipient))
        );

        let confirmed_block = node
            .mine_once(
                default_miner_destination(),
                DEVNET_GENESIS_TIMESTAMP + 101 * 60,
                DEFAULT_MINING_ATTEMPTS,
            )
            .unwrap();
        assert_eq!(confirmed_block.transactions.len(), 1);
        assert_eq!(
            hex::encode(confirmed_block.transactions[0].txid()),
            body["txid"]
        );
        assert!(node.mempool.is_empty());
        let confirmed = node.wallet_snapshot().unwrap();
        let tx_history = confirmed
            .history
            .iter()
            .find(|item| item.txid == body["txid"])
            .unwrap();
        assert_eq!(tx_history.status, "confirmed");
        assert_eq!(tx_history.height, Some(101));
        assert_eq!(tx_history.fee_burned_atoms, "1");
        drop(node);

        let replayed = Node::open(&path).unwrap();
        assert_eq!(replayed.wallet_snapshot().unwrap(), confirmed);
        drop(replayed);
        for entry in fs::read_dir(&path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                let bytes = fs::read(entry.path()).unwrap();
                assert!(!bytes.windows(32).any(|window| window == [0x13; 32]));
            }
        }
        clean_test_dir(&path);
    }

    #[test]
    fn wallet_send_rejects_invalid_requests_without_mempool_mutation() {
        let path = test_dir("wallet-send-invalid");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let recipient = hex::encode(insecure_dev_destination(0x71));

        for (value, expected_status) in [
            (
                json!({"recipient": "00", "amount": "1", "fee": "0.00000001"}),
                400,
            ),
            (
                json!({"recipient": recipient, "amount": "0", "fee": "0.00000001"}),
                400,
            ),
            (
                json!({"recipient": recipient, "amount": "1.000000001", "fee": "0.00000001"}),
                400,
            ),
            (
                json!({"recipient": recipient, "amount": "184467440737.09551616", "fee": "0.00000001"}),
                400,
            ),
            (
                json!({"recipient": recipient, "amount": "1", "fee": "0.00000001", "extra": true}),
                400,
            ),
            (
                json!({"recipient": recipient, "amount": "1", "fee": "0.00000001"}),
                422,
            ),
        ] {
            let response = route_rpc_request(wallet_post("/v1/wallet/send", value), &mut node);
            assert_eq!(response.status, expected_status);
            assert!(node.mempool.is_empty());
        }

        mine_default_chain_to(&mut node, 1);
        let immature = route_rpc_request(
            wallet_post(
                "/v1/wallet/send",
                json!({"recipient": recipient, "amount": "1", "fee": "0.00000001"}),
            ),
            &mut node,
        );
        assert_eq!(immature.status, 422);
        assert!(
            String::from_utf8(immature.body)
                .unwrap()
                .contains("immature")
        );

        let insufficient = route_rpc_request(
            wallet_post(
                "/v1/wallet/send",
                json!({"recipient": recipient, "amount": "1000", "fee": "0.00000001"}),
            ),
            &mut node,
        );
        assert_eq!(insufficient.status, 422);
        assert!(
            String::from_utf8(insufficient.body)
                .unwrap()
                .contains("insufficient")
        );

        mine_default_chain_to(&mut node, 100);
        let too_low = route_rpc_request(
            wallet_post(
                "/v1/wallet/send",
                json!({"recipient": recipient, "amount": "1", "fee": "0"}),
            ),
            &mut node,
        );
        assert_eq!(too_low.status, 422);
        assert!(
            String::from_utf8(too_low.body)
                .unwrap()
                .contains("relay minimum")
        );
        assert!(node.mempool.is_empty());

        let wrong_type = route_rpc_request(
            RpcRequest {
                method: "POST".to_owned(),
                target: "/v1/wallet/send".to_owned(),
                content_type: Some("text/plain".to_owned()),
                body: b"{}".to_vec(),
            },
            &mut node,
        );
        assert_eq!(wrong_type.status, 415);

        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn wallet_history_marks_external_payments_received_and_then_confirmed() {
        let path = test_dir("wallet-received");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let funding = mine_default_chain_to(&mut node, 1).remove(0);
        let received = spend_coinbase_output(&node, &funding, 1, 0x11, 0x13, 10);
        let received_txid = received.txid();
        let received_atoms = received.outputs[0].value;
        node.submit_transaction(received).unwrap();

        let pending = node.wallet_snapshot().unwrap();
        let item = pending
            .history
            .iter()
            .find(|item| item.txid == hex::encode(received_txid))
            .unwrap();
        assert_eq!(item.kind, "received");
        assert_eq!(item.status, "pending");
        assert_eq!(item.net_amount_atoms, received_atoms.to_string());
        assert_eq!(item.fee_burned_atoms, "10");
        assert_eq!(
            item.counterparty,
            Some(hex::encode(insecure_dev_destination(0x11)))
        );

        node.mine_once(
            default_miner_destination(),
            DEVNET_GENESIS_TIMESTAMP + 120,
            DEFAULT_MINING_ATTEMPTS,
        )
        .unwrap();
        let confirmed = node.wallet_snapshot().unwrap();
        let item = confirmed
            .history
            .iter()
            .find(|item| item.txid == hex::encode(received_txid))
            .unwrap();
        assert_eq!(item.kind, "received");
        assert_eq!(item.status, "confirmed");
        assert_eq!(item.height, Some(2));
        assert_eq!(item.confirmations, 1);

        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn wallet_snapshot_tracks_the_active_branch_after_reorg() {
        let path = test_dir("wallet-reorg");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let active = mine_default_chain_to(&mut node, 1).remove(0);
        assert_eq!(
            node.wallet_snapshot().unwrap().balances.immature_atoms,
            active.coinbase.outputs[0].value.to_string()
        );

        let sibling = mined_child(
            &node,
            DEVNET_GENESIS_HASH,
            DEVNET_GENESIS_TIMESTAMP + 61,
            0x51,
        );
        node.submit_block(sibling.clone(), DEVNET_GENESIS_TIMESTAMP + 61)
            .unwrap();
        assert_eq!(node.state.tip(), active.block_id());
        assert_ne!(node.state.tip(), sibling.block_id());
        let sibling_child = mined_child(
            &node,
            sibling.block_id(),
            DEVNET_GENESIS_TIMESTAMP + 121,
            0x52,
        );
        node.submit_block(sibling_child.clone(), DEVNET_GENESIS_TIMESTAMP + 121)
            .unwrap();
        assert_eq!(node.state.tip(), sibling_child.block_id());

        let reorged = node.wallet_snapshot().unwrap();
        assert_eq!(reorged.accepted_height, 2);
        assert_eq!(reorged.balances.spendable_atoms, "0");
        assert_eq!(reorged.balances.immature_atoms, "0");
        assert_eq!(reorged.balances.pending_atoms, "0");
        assert!(reorged.history.is_empty());

        drop(node);
        clean_test_dir(&path);
    }

    #[test]
    fn wallet_consolidation_uses_at_most_128_smallest_unreserved_mature_outputs() {
        let path = test_dir("wallet-consolidate");
        clean_test_dir(&path);
        let mut node = Node::open(&path).unwrap();
        let first = mine_default_chain_to(&mut node, 100).remove(0);
        let first_outpoint = OutPoint {
            txid: first.coinbase_outpoint_id(),
            index: 0,
        };
        let first_value = first.coinbase.outputs[0].value;
        let split_fee = 1_000_u64;
        let split_total = first_value - split_fee;
        let per_output = split_total / MAX_TRANSACTION_INPUTS as u64;
        let remainder = split_total % MAX_TRANSACTION_INPUTS as u64;
        let mut split = Transaction {
            network_id: node.params.network_id,
            version: TRANSACTION_VERSION,
            inputs: vec![TxInput {
                previous: first_outpoint,
                witness: InputWitness::Key {
                    public_key: [0; 32],
                    signature: Vec::new(),
                },
            }],
            outputs: (0..MAX_TRANSACTION_INPUTS)
                .map(|index| TxOutput {
                    value: per_output + u64::from(index == 0) * remainder,
                    lock: OutputLock::Key(default_miner_destination()),
                    spendable_height: node.state.next_height(),
                })
                .collect(),
        };
        let key = insecure_dev_wallet_signing_key();
        split.sign_all(&[&key]).unwrap();
        node.submit_transaction(split).unwrap();
        node.mine_once(
            default_miner_destination(),
            DEVNET_GENESIS_TIMESTAMP + 101 * 60,
            DEFAULT_MINING_ATTEMPTS,
        )
        .unwrap();
        let before = node.wallet_snapshot().unwrap();
        assert_eq!(before.spendable_utxo_count, MAX_TRANSACTION_INPUTS + 1);
        assert!(before.immature_utxo_count > 0);
        assert_eq!(before.reserved_utxo_count, 0);

        assert!(matches!(
            node.consolidate_dev_wallet(1_000, 1),
            Err(NodeError::InvalidConsolidationMaxInputs)
        ));
        assert!(matches!(
            node.consolidate_dev_wallet(1_000, MAX_TRANSACTION_INPUTS + 1),
            Err(NodeError::InvalidConsolidationMaxInputs)
        ));
        assert!(matches!(
            node.consolidate_dev_wallet(0, MAX_TRANSACTION_INPUTS),
            Err(NodeError::MempoolFeeTooLow { .. })
        ));
        assert!(node.mempool.is_empty());

        let invalid_max = route_rpc_request(
            wallet_post(
                "/v1/wallet/consolidate",
                json!({"fee": "0.00001000", "max_inputs": 1}),
            ),
            &mut node,
        );
        assert_eq!(invalid_max.status, 400);
        let response = route_rpc_request(
            wallet_post(
                "/v1/wallet/consolidate",
                json!({"fee": "0.00001000", "max_inputs": MAX_TRANSACTION_INPUTS}),
            ),
            &mut node,
        );
        assert_eq!(response.status, 200);
        let response_body: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert_eq!(response_body["inputs_consolidated"], MAX_TRANSACTION_INPUTS);
        assert_eq!(response_body["fee_burned_atoms"], "1000");
        for field in ["input_atoms", "output_atoms", "fee_burned_atoms"] {
            assert!(response_body[field].is_string());
        }
        let entry = node.mempool.values().next().unwrap();
        assert_eq!(response_body["txid"], hex::encode(entry.txid));
        assert_eq!(entry.transaction.inputs.len(), MAX_TRANSACTION_INPUTS);
        assert_eq!(entry.transaction.outputs.len(), 1);
        assert_eq!(
            entry.transaction.outputs[0].lock,
            OutputLock::Key(default_miner_destination())
        );
        let input_atoms = response_body["input_atoms"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let output_atoms = response_body["output_atoms"]
            .as_str()
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert_eq!(input_atoms, split_total);
        assert_eq!(input_atoms, output_atoms + 1_000);
        assert_eq!(entry.transaction.outputs[0].value, output_atoms);

        let reserved = node.wallet_snapshot().unwrap();
        assert_eq!(reserved.reserved_utxo_count, MAX_TRANSACTION_INPUTS);
        assert_eq!(reserved.spendable_utxo_count, 1);
        assert_eq!(reserved.balances.pending_atoms, output_atoms.to_string());
        assert!(matches!(
            node.consolidate_dev_wallet(1_000, MAX_TRANSACTION_INPUTS),
            Err(NodeError::WalletNotEnoughUtxos(1))
        ));
        let serialized = String::from_utf8(response.body).unwrap();
        assert!(!serialized.contains(&hex::encode([0x13; 32])));

        drop(node);
        clean_test_dir(&path);
    }
}
