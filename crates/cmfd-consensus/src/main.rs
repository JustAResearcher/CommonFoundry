use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use cmfd_consensus::forgematrix::CANDIDATE_16GB_PROFILE;
use cmfd_consensus::forgematrix::target_with_leading_zero_bits;
use cmfd_consensus::{
    BlockChallenge, DEFAULT_MONETARY_POLICY, ForgeMatrixVerifier, TEST_PROFILE, v2_test_reference,
};

#[derive(Debug, Parser)]
#[command(
    name = "cmfd-consensus",
    version,
    about = "CommonFoundry consensus reference tools"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Emit a deterministic ForgeMatrix test vector.
    Vector,
    /// Mine the small CPU test profile.
    Mine {
        #[arg(long, default_value_t = 12)]
        leading_zero_bits: u16,
        #[arg(long, default_value_t = 100_000)]
        attempts: u64,
    },
    /// Show the subsidy, allocation, and burned fees at a height.
    Economics {
        #[arg(long)]
        height: u64,
        #[arg(long, default_value_t = 0)]
        fees: u64,
    },
    /// Write a binary fixture for the independent CUDA differential test.
    GpuFixture {
        #[arg(long)]
        output: std::path::PathBuf,
        #[arg(long, default_value_t = 7)]
        nonce: u64,
    },
    /// Write the explicit-byte, mod-251 v2 CUDA differential fixture.
    GpuFixtureV2 {
        #[arg(long)]
        output: std::path::PathBuf,
        #[arg(long, default_value_t = 7)]
        nonce: u64,
    },
    /// Report exact memory and work for the unactivated 16 GB candidate.
    Profile16gb,
}

fn sample_block(network_id: [u8; 32], target: [u8; 32]) -> BlockChallenge {
    BlockChallenge {
        network_id,
        previous_block: [0x11; 32],
        transaction_root: [0x22; 32],
        height: 42,
        timestamp: 1_777_777_777,
        target,
    }
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Vector => {
            let verifier = ForgeMatrixVerifier::new(TEST_PROFILE)?;
            let block = sample_block([0x33; 32], [0xff; 32]);
            let proof = verifier.prove(&block, 7);
            verifier.verify(&block, &proof)?;
            println!("{}", serde_json::to_string_pretty(&proof)?);
        }
        Command::Mine {
            leading_zero_bits,
            attempts,
        } => {
            let verifier = ForgeMatrixVerifier::new(TEST_PROFILE)?;
            let block = sample_block([0x33; 32], target_with_leading_zero_bits(leading_zero_bits));
            let proof = verifier
                .mine(&block, 0, attempts)
                .with_context(|| format!("no solution in {attempts} attempts"))?;
            verifier.verify(&block, &proof)?;
            println!("{}", serde_json::to_string_pretty(&proof)?);
        }
        Command::Economics { height, fees } => {
            let allocation = DEFAULT_MONETARY_POLICY.allocation(height, fees)?;
            println!("{}", serde_json::to_string_pretty(&allocation)?);
        }
        Command::GpuFixture { output, nonce } => {
            let verifier = ForgeMatrixVerifier::new(TEST_PROFILE)?;
            let fixture = verifier.gpu_fixture(&sample_block([0x33; 32], [0xff; 32]), nonce);
            std::fs::write(&output, fixture)
                .with_context(|| format!("failed to write {}", output.display()))?;
            println!("wrote {}", output.display());
        }
        Command::GpuFixtureV2 { output, nonce } => {
            let oracle = v2_test_reference()?;
            let fixture = oracle.gpu_fixture(
                &sample_block(oracle.descriptor().network_id, [0xff; 32]),
                nonce,
            )?;
            std::fs::write(&output, fixture)
                .with_context(|| format!("failed to write {}", output.display()))?;
            println!("wrote {}", output.display());
        }
        Command::Profile16gb => {
            println!(
                "{}",
                serde_json::to_string_pretty(&CANDIDATE_16GB_PROFILE.metrics())?
            );
        }
    }
    Ok(())
}
