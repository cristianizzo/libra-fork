// Copyright (c) The Libra Core Contributors
// SPDX-License-Identifier: Apache-2.0
#![allow(dead_code)]

mod block_processor;
#[cfg(test)]
mod executor_test;
#[cfg(test)]
mod mock_vm;

use crate::block_processor::BlockProcessor;
use failure::{format_err, Result};
use futures::channel::oneshot;
use futures::executor::block_on;
use lazy_static::lazy_static;
use libra_config::config::NodeConfig;
use libra_crypto::{
    hash::{
        EventAccumulatorHasher, TransactionAccumulatorHasher, ACCUMULATOR_PLACEHOLDER_HASH,
        SPARSE_MERKLE_PLACEHOLDER_HASH,
    },
    HashValue,
};
use libra_logger::prelude::*;

use libra_types::{
    account_address::AccountAddress,
    account_state_blob::AccountStateBlob,
    contract_event::ContractEvent,
    crypto_proxies::LedgerInfoWithSignatures,
    ledger_info::LedgerInfo,
    proof::accumulator::InMemoryAccumulator,
    transaction::{Transaction, TransactionListWithProof, TransactionStatus, Version},
    validator_set::ValidatorSet,
};
use scratchpad::SparseMerkleTree;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::{
    marker::PhantomData,
    sync::{mpsc, Arc, Mutex},
};
use storage_client::{StorageRead, StorageWrite};
use vm_runtime::VMExecutor;

lazy_static! {
    static ref OP_COUNTERS: libra_metrics::OpMetrics =
        libra_metrics::OpMetrics::new_and_registered("executor");
}

/// A structure that summarizes the result of the execution needed for consensus to agree on.
/// The execution is responsible for generating the ID of the new state, which is returned in the
/// result.
///
/// Not every transaction in the payload succeeds: the returned vector keeps the boolean status
/// of success / failure of the transactions.
/// Note that the specific details of compute_status are opaque to StateMachineReplication,
/// which is going to simply pass the results between StateComputer and TxnManager.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub struct StateComputeResult {
    pub executed_state: ExecutedState,
    /// The compute status (success/failure) of the given payload. The specific details are opaque
    /// for StateMachineReplication, which is merely passing it between StateComputer and
    /// TxnManager.
    pub compute_status: Vec<TransactionStatus>,
}

impl StateComputeResult {
    pub fn version(&self) -> Version {
        self.executed_state.version
    }

    pub fn root_hash(&self) -> HashValue {
        self.executed_state.state_id
    }

    pub fn status(&self) -> &Vec<TransactionStatus> {
        &self.compute_status
    }

    pub fn has_reconfiguration(&self) -> bool {
        self.executed_state.validators.is_some()
    }
}

/// Executed state derived from StateComputeResult that is maintained with every proposed block.
/// `state_id`(transaction accumulator root hash) summarized both the information of the version and
/// the validators.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutedState {
    /// Tracks the execution state of a proposed block
    pub state_id: HashValue,
    /// Version of after executing a proposed block.  This state must be persisted to ensure
    /// that on restart that the version is calculated correctly
    pub version: Version,
    /// If set, this is the validator set that should be changed to if this block is committed.
    /// TODO [Reconfiguration] the validators are currently ignored, no reconfiguration yet.
    pub validators: Option<ValidatorSet>,
}

impl ExecutedState {
    pub fn state_for_genesis() -> Self {
        ExecutedState {
            state_id: *ACCUMULATOR_PLACEHOLDER_HASH,
            version: 0,
            validators: None,
        }
    }
}

/// The entire set of data associated with a transaction. In addition to the output generated by VM
/// which includes the write set and events, this also has the in-memory trees.
#[derive(Clone, Debug)]
pub struct TransactionData {
    /// Each entry in this map represents the new blob value of an account touched by this
    /// transaction. The blob is obtained by deserializing the previous blob into a BTreeMap,
    /// applying relevant portion of write set on the map and serializing the updated map into a
    /// new blob.
    account_blobs: HashMap<AccountAddress, AccountStateBlob>,

    /// The list of events emitted during this transaction.
    events: Vec<ContractEvent>,

    /// The execution status set by the VM.
    status: TransactionStatus,

    /// The in-memory Sparse Merkle Tree after the write set is applied. This is `Rc` because the
    /// tree has uncommitted state and sometimes `StateVersionView` needs to have a pointer to the
    /// tree so VM can read it.
    state_tree: Arc<SparseMerkleTree>,

    /// The in-memory Merkle Accumulator that has all events emitted by this transaction.
    event_tree: Arc<InMemoryAccumulator<EventAccumulatorHasher>>,

    /// The amount of gas used.
    gas_used: u64,

    /// The number of newly created accounts.
    num_account_created: usize,

    /// The transaction info hash if the VM status output was keep, None otherwise
    txn_info_hash: Option<HashValue>,
}

impl TransactionData {
    fn new(
        account_blobs: HashMap<AccountAddress, AccountStateBlob>,
        events: Vec<ContractEvent>,
        status: TransactionStatus,
        state_tree: Arc<SparseMerkleTree>,
        event_tree: Arc<InMemoryAccumulator<EventAccumulatorHasher>>,
        gas_used: u64,
        num_account_created: usize,
        txn_info_hash: Option<HashValue>,
    ) -> Self {
        TransactionData {
            account_blobs,
            events,
            status,
            state_tree,
            event_tree,
            gas_used,
            num_account_created,
            txn_info_hash,
        }
    }

    fn account_blobs(&self) -> &HashMap<AccountAddress, AccountStateBlob> {
        &self.account_blobs
    }

    fn events(&self) -> &[ContractEvent] {
        &self.events
    }

    fn status(&self) -> &TransactionStatus {
        &self.status
    }

    fn state_root_hash(&self) -> HashValue {
        self.state_tree.root_hash()
    }

    fn event_root_hash(&self) -> HashValue {
        self.event_tree.root_hash()
    }

    fn gas_used(&self) -> u64 {
        self.gas_used
    }

    fn num_account_created(&self) -> usize {
        self.num_account_created
    }

    fn prune_state_tree(&self) {
        self.state_tree.prune()
    }

    pub fn txn_info_hash(&self) -> Option<HashValue> {
        self.txn_info_hash
    }
}

/// Generated by processing VM's output.
#[derive(Debug, Clone)]
pub struct ProcessedVMOutput {
    /// The entire set of data associated with each transaction.
    transaction_data: Vec<TransactionData>,

    /// The in-memory Merkle Accumulator and state Sparse Merkle Tree after appending all the
    /// transactions in this set.
    executed_trees: ExecutedTrees,

    /// If set, this is the validator set that should be changed to if this block is committed.
    /// TODO [Reconfiguration] the validators are currently ignored, no reconfiguration yet.
    validators: Option<ValidatorSet>,
}

impl ProcessedVMOutput {
    pub fn new(
        transaction_data: Vec<TransactionData>,
        executed_trees: ExecutedTrees,
        validators: Option<ValidatorSet>,
    ) -> Self {
        ProcessedVMOutput {
            transaction_data,
            executed_trees,
            validators,
        }
    }

    pub fn transaction_data(&self) -> &[TransactionData] {
        &self.transaction_data
    }

    pub fn executed_trees(&self) -> &ExecutedTrees {
        &self.executed_trees
    }

    pub fn accu_root(&self) -> HashValue {
        self.executed_trees().txn_accumulator().root_hash()
    }

    pub fn version(&self) -> Option<Version> {
        self.executed_trees().version()
    }

    pub fn validators(&self) -> &Option<ValidatorSet> {
        &self.validators
    }

    // This method should only be called by tests.
    pub fn set_validators(&mut self, validator_set: ValidatorSet) {
        self.validators = Some(validator_set)
    }

    pub fn state_compute_result(&self) -> StateComputeResult {
        let num_leaves = self.executed_trees().txn_accumulator().num_leaves();
        let version = if num_leaves == 0 { 0 } else { num_leaves - 1 };
        StateComputeResult {
            // Now that we have the root hash and execution status we can send the response to
            // consensus.
            // TODO: The VM will support a special transaction to set the validators for the
            // next epoch that is part of a block execution.
            executed_state: ExecutedState {
                state_id: self.accu_root(),
                version,
                validators: self.validators.clone(),
            },
            compute_status: self
                .transaction_data()
                .iter()
                .map(|txn_data| txn_data.status())
                .cloned()
                .collect(),
        }
    }
}

/// `Executor` implements all functionalities the execution module needs to provide.
pub struct Executor<V> {
    /// A thread that keeps processing blocks.
    block_processor_thread: Option<std::thread::JoinHandle<()>>,

    /// Where we can send command to the block processor. The block processor sits at the other end
    /// of the channel and processes the commands.
    command_sender: Mutex<Option<mpsc::Sender<Command>>>,

    committed_trees: Arc<Mutex<ExecutedTrees>>,

    phantom: PhantomData<V>,
}

impl<V> Executor<V>
where
    V: VMExecutor,
{
    /// Constructs an `Executor`.
    pub fn new(
        storage_read_client: Arc<dyn StorageRead>,
        storage_write_client: Arc<dyn StorageWrite>,
        config: &NodeConfig,
    ) -> Self {
        let (command_sender, command_receiver) = mpsc::channel();

        let startup_info = storage_read_client
            .get_startup_info()
            .expect("Failed to read startup info from storage.");

        let (committed_trees, synced_trees, committed_timestamp_usecs) = match startup_info {
            Some(info) => {
                info!("Startup info read from DB: {:?}.", info);
                let ledger_info = info.ledger_info;
                (
                    ExecutedTrees::new(
                        info.committed_tree_state.account_state_root_hash,
                        info.committed_tree_state.ledger_frozen_subtree_hashes,
                        info.committed_tree_state.version + 1,
                    ),
                    info.synced_tree_state.map(|state| {
                        ExecutedTrees::new(
                            state.account_state_root_hash,
                            state.ledger_frozen_subtree_hashes,
                            state.version + 1,
                        )
                    }),
                    ledger_info.ledger_info().timestamp_usecs(),
                )
            }
            None => {
                info!("Startup info is empty. Will start from GENESIS.");
                (ExecutedTrees::new_empty(), None, 0)
            }
        };
        let committed_trees = Arc::new(Mutex::new(committed_trees));

        let vm_config = config.vm_config.clone();
        let genesis_txn = config
            .get_genesis_transaction()
            .expect("failed to load genesis transaction!");
        let cloned_committed_trees = committed_trees.clone();
        let (resp_sender, resp_receiver) = oneshot::channel();
        let executor = Executor {
            block_processor_thread: Some(
                std::thread::Builder::new()
                    .name("block_processor".into())
                    .spawn(move || {
                        let mut block_processor = BlockProcessor::<V>::new(
                            command_receiver,
                            storage_read_client,
                            storage_write_client,
                            cloned_committed_trees,
                            synced_trees,
                            committed_timestamp_usecs,
                            vm_config,
                            genesis_txn,
                            resp_sender,
                        );
                        block_processor.run();
                    })
                    .expect("Failed to create block processor thread."),
            ),
            command_sender: Mutex::new(Some(command_sender)),
            phantom: PhantomData,
            committed_trees,
        };
        block_on(resp_receiver).expect("initialization is done");
        executor
    }

    /// Executes a block.
    pub fn execute_block(
        &self,
        transactions: Vec<Transaction>,
        parent_trees: ExecutedTrees,
        parent_id: HashValue,
        id: HashValue,
    ) -> oneshot::Receiver<Result<ProcessedVMOutput>> {
        debug!(
            "Received request to execute block. Parent id: {:x}. Id: {:x}.",
            parent_id, id
        );

        let (resp_sender, resp_receiver) = oneshot::channel();
        match self
            .command_sender
            .lock()
            .expect("Failed to lock mutex.")
            .as_ref()
        {
            Some(sender) => sender
                .send(Command::ExecuteBlock {
                    executable_block: ExecutableBlock {
                        transactions,
                        parent_trees,
                        parent_id,
                        id,
                    },
                    resp_sender,
                })
                .expect("Did block processor thread panic?"),
            None => resp_sender
                .send(Err(format_err!("Executor is shutting down.")))
                .expect("Failed to send error message."),
        }
        resp_receiver
    }

    /// Commits a block and all its ancestors within a block batch. Returns `Ok(())` if successful.
    pub fn commit_blocks(
        &self,
        blocks: Vec<CommittableBlock>,
        ledger_info_with_sigs: LedgerInfoWithSignatures,
    ) -> oneshot::Receiver<Result<()>> {
        debug!(
            "Received request to commit block {:x}.",
            ledger_info_with_sigs.ledger_info().consensus_block_id()
        );

        let (resp_sender, resp_receiver) = oneshot::channel();
        // TODO: check li_sigs's consensus id matches the last block.
        match self
            .command_sender
            .lock()
            .expect("Failed to lock mutex.")
            .as_ref()
        {
            Some(sender) => sender
                .send(Command::CommitBlockBatch {
                    committable_block_batch: CommittableBlockBatch {
                        blocks,
                        finality_proof: ledger_info_with_sigs,
                    },
                    resp_sender,
                })
                .expect("Did block processor thread panic?"),
            None => resp_sender
                .send(Err(format_err!("Executor is shutting down.")))
                .expect("Failed to send error message."),
        }
        resp_receiver
    }

    /// Executes and commits a chunk of transactions that are already committed by majority of the
    /// validators.
    pub fn execute_and_commit_chunk(
        &self,
        txn_list_with_proof: TransactionListWithProof,
        ledger_info_with_sigs: LedgerInfoWithSignatures,
    ) -> oneshot::Receiver<Result<()>> {
        debug!(
            "Received request to execute chunk. Chunk size: {}. Target version: {}.",
            txn_list_with_proof.transactions.len(),
            ledger_info_with_sigs.ledger_info().version(),
        );

        let (resp_sender, resp_receiver) = oneshot::channel();
        match self
            .command_sender
            .lock()
            .expect("Failed to lock mutex.")
            .as_ref()
        {
            Some(sender) => sender
                .send(Command::ExecuteAndCommitChunk {
                    chunk: Chunk {
                        txn_list_with_proof,
                        ledger_info_with_sigs,
                    },
                    resp_sender,
                })
                .expect("Did block processor thread panic?"),
            None => resp_sender
                .send(Err(format_err!("Executor is shutting down.")))
                .expect("Failed to send error message."),
        }
        resp_receiver
    }

    pub fn committed_trees(&self) -> ExecutedTrees {
        (*self.committed_trees.lock().unwrap()).clone()
    }
}

impl<V> Drop for Executor<V> {
    fn drop(&mut self) {
        // Drop the sender so the block processor thread will exit.
        self.command_sender
            .lock()
            .expect("Failed to lock mutex.")
            .take()
            .expect("Command sender should exist.");
        self.block_processor_thread
            .take()
            .expect("Block processor thread should exist.")
            .join()
            .expect("Did block processor thread panic?");
    }
}

#[derive(Debug)]
struct CommittableBlockBatch {
    blocks: Vec<CommittableBlock>,
    finality_proof: LedgerInfoWithSignatures,
}

#[derive(Debug)]
pub struct CommittableBlock {
    transactions: Vec<Transaction>,
    output: Arc<ProcessedVMOutput>,
}

impl CommittableBlock {
    pub fn new(transactions: Vec<Transaction>, output: Arc<ProcessedVMOutput>) -> Self {
        Self {
            transactions,
            output,
        }
    }
}

#[derive(Debug)]
struct ExecutableBlock {
    id: HashValue,
    parent_id: HashValue,
    parent_trees: ExecutedTrees,
    transactions: Vec<Transaction>,
}

#[derive(Clone, Debug)]
struct Chunk {
    txn_list_with_proof: TransactionListWithProof,
    ledger_info_with_sigs: LedgerInfoWithSignatures,
}

impl Chunk {
    fn ledger_info(&self) -> &LedgerInfo {
        self.ledger_info_with_sigs.ledger_info()
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
enum Command {
    ExecuteBlock {
        executable_block: ExecutableBlock,
        resp_sender: oneshot::Sender<Result<ProcessedVMOutput>>,
    },
    CommitBlockBatch {
        committable_block_batch: CommittableBlockBatch,
        resp_sender: oneshot::Sender<Result<()>>,
    },
    ExecuteAndCommitChunk {
        chunk: Chunk,
        resp_sender: oneshot::Sender<Result<()>>,
    },
}

#[derive(Clone, Debug)]
pub struct ExecutedTrees {
    /// The in-memory Sparse Merkle Tree representing a specific state after execution. If this
    /// tree is presenting the latest commited state, it will have a single Subtree node (or
    /// Empty node) whose hash equals the root hash of the newest Sparse Merkle Tree in
    /// storage.
    state_tree: Arc<SparseMerkleTree>,

    /// The in-memory Merkle Accumulator representing a blockchain state consistent with the
    /// `state_tree`.
    transaction_accumulator: Arc<InMemoryAccumulator<TransactionAccumulatorHasher>>,
}

impl ExecutedTrees {
    pub fn state_tree(&self) -> &Arc<SparseMerkleTree> {
        &self.state_tree
    }

    pub fn txn_accumulator(&self) -> &Arc<InMemoryAccumulator<TransactionAccumulatorHasher>> {
        &self.transaction_accumulator
    }

    pub fn version(&self) -> Option<Version> {
        let num_elements = self.txn_accumulator().num_leaves() as u64;
        if num_elements > 0 {
            Some(num_elements - 1)
        } else {
            None
        }
    }

    pub fn state_id(&self) -> HashValue {
        self.txn_accumulator().root_hash()
    }

    pub fn state_root(&self) -> HashValue {
        self.state_tree().root_hash()
    }

    pub fn new(
        state_root_hash: HashValue,
        frozen_subtrees_in_accumulator: Vec<HashValue>,
        num_leaves_in_accumulator: u64,
    ) -> ExecutedTrees {
        ExecutedTrees {
            state_tree: Arc::new(SparseMerkleTree::new(state_root_hash)),
            transaction_accumulator: Arc::new(
                InMemoryAccumulator::new(frozen_subtrees_in_accumulator, num_leaves_in_accumulator)
                    .expect("The startup info read from storage should be valid."),
            ),
        }
    }

    pub fn new_empty() -> ExecutedTrees {
        Self::new(*SPARSE_MERKLE_PLACEHOLDER_HASH, vec![], 0)
    }
}
