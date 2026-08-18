use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Parser, Subcommand};
use cmfd_node::p2p::{spawn_inbound_listener, spawn_static_peer_polling};
use cmfd_node::peer::{PeerLimits, StaticPeerConfig};
use cmfd_node::{
    DEFAULT_DATA_DIR, DEFAULT_MINING_ATTEMPTS, DEFAULT_P2P_ADDRESS, DEFAULT_RPC_ADDRESS, Node,
    default_miner_destination, parse_miner_destination, serve_rpc_shared, unix_time_seconds,
};
use serde_json::json;

#[derive(Debug, Parser)]
#[command(
    name = "cmfd-node",
    version,
    about = "CommonFoundry private multi-node Devnet-0 runtime"
)]
struct Cli {
    #[arg(long, global = true, default_value = DEFAULT_DATA_DIR)]
    data_dir: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the loopback RPC and private-network P2P services.
    Run {
        #[arg(long, default_value = DEFAULT_RPC_ADDRESS)]
        bind: SocketAddr,
        #[arg(long, default_value = DEFAULT_P2P_ADDRESS)]
        p2p_bind: SocketAddr,
        /// Static private peer address. Repeat for multiple peers.
        #[arg(long = "peer")]
        peers: Vec<SocketAddr>,
    },
    /// Mine, validate, persist, and apply one Devnet-0 block locally.
    MineOnce {
        /// 32-byte x-only Schnorr public key as 64 hex characters.
        #[arg(long)]
        miner: Option<String>,
        #[arg(long, default_value_t = DEFAULT_MINING_ATTEMPTS)]
        attempts: u64,
    },
    /// Replay the block log and print current offline node status.
    Status,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run {
            bind,
            p2p_bind,
            peers,
        } => {
            let node = Node::open(&cli.data_dir)?;
            let status = node.status()?;
            let shared = Arc::new(Mutex::new(node));
            let limits = PeerLimits::default();
            let p2p_socket = TcpListener::bind(p2p_bind)?;
            let p2p_address = p2p_socket.local_addr()?;
            let inbound = spawn_inbound_listener(Arc::clone(&shared), p2p_socket, limits)?;
            let poller = if peers.is_empty() {
                None
            } else {
                Some(spawn_static_peer_polling(
                    Arc::clone(&shared),
                    StaticPeerConfig {
                        listen_address: p2p_address,
                        peers: peers.clone(),
                        limits,
                    },
                    Duration::from_secs(2),
                )?)
            };
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "rpc": bind.to_string(),
                    "p2p": p2p_address.to_string(),
                    "static_peers": peers.iter().map(ToString::to_string).collect::<Vec<_>>(),
                    "status": status,
                    "warning": "private, valueless Devnet-0; peer compatibility is not identity authentication or encryption"
                }))?
            );
            let rpc_result = serve_rpc_shared(shared, bind);
            let poll_result = match poller {
                Some(poller) => poller.stop(),
                None => Ok(()),
            };
            let inbound_result = inbound.stop();
            rpc_result?;
            poll_result?;
            inbound_result?;
            Ok(())
        }
        Command::MineOnce { miner, attempts } => {
            let miner_destination = match miner.as_deref() {
                Some(value) => parse_miner_destination(value)?,
                None => default_miner_destination(),
            };
            let mut node = Node::open(&cli.data_dir)?;
            let block = node.mine_once(miner_destination, unix_time_seconds()?, attempts)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "accepted": true,
                    "height": block.challenge.height,
                    "block_id": hex::encode(block.block_id()),
                    "proof_type": "forgematrix-v2-reference",
                    "miner": hex::encode(miner_destination),
                    "used_insecure_default_miner": miner.is_none(),
                    "status": node.status()?,
                }))?
            );
            Ok(())
        }
        Command::Status => {
            let node = Node::open(&cli.data_dir)?;
            println!("{}", serde_json::to_string_pretty(&node.status()?)?);
            Ok(())
        }
    }
}
