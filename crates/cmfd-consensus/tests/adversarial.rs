use cmfd_consensus::forgematrix::target_with_leading_zero_bits;
use cmfd_consensus::{
    BlockChallenge, CoinbaseClaim, DEFAULT_MONETARY_POLICY, EconomicsError, ForgeMatrixError,
    ForgeMatrixVerifier, TEST_PROFILE,
};

fn block() -> BlockChallenge {
    BlockChallenge {
        network_id: [0x44; 32],
        previous_block: [0x31; 32],
        transaction_root: [0x72; 32],
        height: 9001,
        timestamp: 1_777_000_001,
        target: [0xff; 32],
    }
}

#[test]
fn every_header_field_is_bound() {
    let verifier = ForgeMatrixVerifier::new(TEST_PROFILE).unwrap();
    let original = block();
    let proof = verifier.prove(&original, 123);
    assert!(verifier.verify(&original, &proof).is_ok());

    let mut mutations = Vec::new();
    let mut b = original;
    b.network_id[0] ^= 1;
    mutations.push(b);
    let mut b = original;
    b.previous_block[0] ^= 1;
    mutations.push(b);
    let mut b = original;
    b.transaction_root[0] ^= 1;
    mutations.push(b);
    let mut b = original;
    b.height += 1;
    mutations.push(b);
    let mut b = original;
    b.timestamp += 1;
    mutations.push(b);
    let mut b = original;
    b.target[0] ^= 1;
    mutations.push(b);

    for mutated in mutations {
        assert!(matches!(
            verifier.verify(&mutated, &proof),
            Err(ForgeMatrixError::OutputDigest
                | ForgeMatrixError::WorkDigest
                | ForgeMatrixError::HighHash)
        ));
    }
}

#[test]
fn proof_fields_cannot_be_substituted() {
    let verifier = ForgeMatrixVerifier::new(TEST_PROFILE).unwrap();
    let block = block();
    let proof = verifier.prove(&block, 55);

    let mut tampered = proof.clone();
    tampered.nonce += 1;
    assert_eq!(
        verifier.verify(&block, &tampered),
        Err(ForgeMatrixError::OutputDigest)
    );

    let mut tampered = proof.clone();
    tampered.model_root[0] ^= 1;
    assert_eq!(
        verifier.verify(&block, &tampered),
        Err(ForgeMatrixError::ModelRoot)
    );

    let mut tampered = proof.clone();
    tampered.output_digest[0] ^= 1;
    assert_eq!(
        verifier.verify(&block, &tampered),
        Err(ForgeMatrixError::OutputDigest)
    );

    let mut tampered = proof;
    tampered.work_digest[0] ^= 1;
    assert_eq!(
        verifier.verify(&block, &tampered),
        Err(ForgeMatrixError::WorkDigest)
    );
}

#[test]
fn work_from_a_shorter_or_different_model_is_rejected() {
    let verifier = ForgeMatrixVerifier::new(TEST_PROFILE).unwrap();
    let mut shortcut_profile = TEST_PROFILE;
    shortcut_profile.layers -= 1;
    shortcut_profile.model_seed[0] ^= 1;
    let shortcut = ForgeMatrixVerifier::new(shortcut_profile).unwrap();
    let block = block();
    let shortcut_proof = shortcut.prove(&block, 55);

    assert_eq!(
        verifier.verify(&block, &shortcut_proof),
        Err(ForgeMatrixError::ModelRoot)
    );

    let mut disguised = shortcut_proof;
    disguised.model_root = verifier.model_root();
    assert_eq!(
        verifier.verify(&block, &disguised),
        Err(ForgeMatrixError::OutputDigest)
    );
}

#[test]
fn every_digest_byte_is_consensus_bound() {
    let verifier = ForgeMatrixVerifier::new(TEST_PROFILE).unwrap();
    let block = block();
    let proof = verifier.prove(&block, 8080);

    for index in 0..32 {
        let mut tampered = proof.clone();
        tampered.model_root[index] ^= 1;
        assert_eq!(
            verifier.verify(&block, &tampered),
            Err(ForgeMatrixError::ModelRoot)
        );

        let mut tampered = proof.clone();
        tampered.output_digest[index] ^= 1;
        assert_eq!(
            verifier.verify(&block, &tampered),
            Err(ForgeMatrixError::OutputDigest)
        );

        let mut tampered = proof.clone();
        tampered.work_digest[index] ^= 1;
        assert_eq!(
            verifier.verify(&block, &tampered),
            Err(ForgeMatrixError::WorkDigest)
        );
    }
}

#[test]
fn a_valid_computation_still_has_to_meet_difficulty() {
    let verifier = ForgeMatrixVerifier::new(TEST_PROFILE).unwrap();
    let mut impossible = block();
    impossible.target = target_with_leading_zero_bits(256);
    let proof = verifier.prove(&impossible, 0);
    assert_eq!(
        verifier.verify(&impossible, &proof),
        Err(ForgeMatrixError::HighHash)
    );
}

#[test]
fn transaction_fees_cannot_be_redirected_to_miner() {
    let policy = DEFAULT_MONETARY_POLICY;
    let height = 1;
    let fees = 9_000_000;
    let expected = policy.allocation(height, fees).unwrap();
    let claim = CoinbaseClaim {
        miner: expected.miner + fees,
        steward: expected.steward,
        community: expected.community,
    };
    assert_eq!(
        policy.validate_coinbase(height, fees, claim),
        Err(EconomicsError::Miner {
            actual: expected.miner + fees,
            expected: expected.miner,
        })
    );
}
