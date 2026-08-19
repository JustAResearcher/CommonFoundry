pub use cmfd_cuda::CudaMiner;

#[cfg(test)]
mod tests {
    use super::*;
    use cmfd_consensus::{BlockChallenge, v2_test_reference};

    #[test]
    fn available_cuda_backend_matches_authoritative_v2_digests() {
        let reference = v2_test_reference().unwrap();
        let descriptor = reference.descriptor();
        let block = BlockChallenge {
            network_id: descriptor.network_id,
            previous_block: [1; 32],
            transaction_root: [2; 32],
            height: 3,
            timestamp: 1_700_000_123,
            target: [0xff; 32],
        };
        let model = reference.accelerator_model();
        let Some(mut backend) = CudaMiner::load(&model).unwrap() else {
            return;
        };
        let batch = reference
            .prepare_accelerator_batch(&block, 91, 128)
            .unwrap();
        let outputs = backend.evaluate(&batch).unwrap();
        for (index, output) in outputs.chunks_exact(batch.activation_len()).enumerate() {
            let accelerated = batch.candidate_work_digest(index, output).unwrap();
            let reference = reference
                .prove_compact(&block, batch.nonce_at(index).unwrap())
                .unwrap();
            assert_eq!(accelerated, reference.work_digest, "nonce index {index}");
        }
    }
}
