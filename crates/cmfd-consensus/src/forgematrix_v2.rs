//! ForgeMatrix v2 integer relation and full-recomputation research oracle.
//!
//! This module deliberately does not expose a production block verifier. It
//! fixes the arithmetic that a future GKR/sumcheck proof must constrain and
//! supplies a small-profile witness checker for differential and adversarial
//! testing. Model bytes come from [`crate::model_bank`], never from a seed.

use blake3::Hasher;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    BlockChallenge,
    model_bank::{
        ModelBankError, ModelBankManifest, SmallModelBankFixture, build_small_model_bank,
    },
};

pub const FORGEMATRIX_V2_ALGORITHM_VERSION: u32 = 2;
pub const FORGEMATRIX_V2_PROOF_VERSION: u32 = 1;
pub const V2_REFERENCE_MAX_DIMENSION: u32 = 32;
pub const V2_REFERENCE_MAX_BATCH: u32 = 4;
pub const V2_REFERENCE_MAX_LAYERS: u32 = 4;

pub const PRODUCTION_V2_DIMENSION: u32 = 4096;
pub const PRODUCTION_V2_BATCH: u32 = 128;
pub const PRODUCTION_V2_LAYERS: u32 = 384;
pub const PRODUCTION_V2_BANKS: u32 = 3;
pub const PRODUCTION_V2_LAYERS_PER_BANK: u32 = 128;
pub const V2_TEST_DIMENSION: u32 = 4;
pub const V2_TEST_BATCH: u32 = 2;
pub const V2_TEST_LAYERS: u32 = 4;

const CHALLENGE_DOMAIN: &str = "CMFD/FORGEMATRIX/CHALLENGE/V2";
const MASK_DOMAIN: &str = "CMFD/FORGEMATRIX/MASKCOEFF/V2";
const OUTPUT_DOMAIN: &str = "CMFD/FORGEMATRIX/OUTPUT/V2";
const WORK_DOMAIN: &str = "CMFD/FORGEMATRIX/WORK/V2";

pub const V2_TRANSITION_MODULUS: u32 = 134_217_689;
const OUTPUT_MODULUS: u64 = 251;
const CENTER: i16 = 125;
const MAX_OUTPUT_QUOTIENT: u32 = 534_731;

/// Consensus-owned public identity of the v2 relation.
///
/// Production construction is intentionally unavailable while the proof
/// backend, PCS linkage certificate, and audit gates remain incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeMatrixV2Descriptor {
    pub network_id: [u8; 32],
    pub algorithm_version: u32,
    pub proof_version: u32,
    pub banks: u32,
    pub layers_per_bank: u32,
    pub model: ModelBankManifest,
}

impl ForgeMatrixV2Descriptor {
    pub fn new_research(
        network_id: [u8; 32],
        banks: u32,
        layers_per_bank: u32,
        model: ModelBankManifest,
    ) -> Result<Self, ForgeMatrixV2Error> {
        let descriptor = Self {
            network_id,
            algorithm_version: FORGEMATRIX_V2_ALGORITHM_VERSION,
            proof_version: FORGEMATRIX_V2_PROOF_VERSION,
            banks,
            layers_per_bank,
            model,
        };
        descriptor.validate_research()?;
        Ok(descriptor)
    }

    pub fn validate_research(&self) -> Result<(), ForgeMatrixV2Error> {
        if self.algorithm_version != FORGEMATRIX_V2_ALGORITHM_VERSION {
            return Err(ForgeMatrixV2Error::AlgorithmVersion);
        }
        if self.proof_version != FORGEMATRIX_V2_PROOF_VERSION {
            return Err(ForgeMatrixV2Error::ProofVersion);
        }
        if self.network_id == [0; 32]
            || self.model.raw_blake3_root == [0; 32]
            || self.model.pcs_parameter_digest == [0; 32]
            || self.model.pcs_commitment_root == [0; 32]
        {
            return Err(ForgeMatrixV2Error::UncommittedDescriptor);
        }
        if !self.model.dimension.is_power_of_two()
            || !self.model.batch.is_power_of_two()
            || !self.layers_per_bank.is_power_of_two()
            || self.banks == 0
            || self
                .banks
                .checked_mul(self.layers_per_bank)
                .is_none_or(|layers| layers != self.model.layers)
        {
            return Err(ForgeMatrixV2Error::InvalidShape);
        }
        if self.model.dimension > V2_REFERENCE_MAX_DIMENSION
            || self.model.batch > V2_REFERENCE_MAX_BATCH
            || self.model.layers > V2_REFERENCE_MAX_LAYERS
        {
            return Err(ForgeMatrixV2Error::ProductionDisabled);
        }
        self.model.digest()?;
        Ok(())
    }
}

/// One exact Euclidean-reduction and cubic-S-box witness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReductionWitness {
    pub z: i32,
    pub encoded_z: u32,
    pub square_quotient: u32,
    pub square_remainder: u32,
    pub cube_quotient: u32,
    pub cube_remainder: u32,
    pub output_quotient: u32,
    pub output_remainder: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerWitness {
    pub accumulators: Vec<i32>,
    pub reductions: Vec<ReductionWitness>,
    pub output: Vec<i16>,
}

/// A deliberately non-succinct witness used only as the security oracle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeMatrixV2ReferenceProof {
    pub nonce: u64,
    pub challenge_digest: [u8; 32],
    pub initial_reductions: Vec<ReductionWitness>,
    pub initial_activation: Vec<i16>,
    pub layers: Vec<LayerWitness>,
    pub final_activation_digest: [u8; 32],
    pub work_digest: [u8; 32],
}

#[derive(Debug, Error)]
pub enum ForgeMatrixV2Error {
    #[error("ForgeMatrix v2 algorithm version mismatch")]
    AlgorithmVersion,
    #[error("ForgeMatrix v2 proof version mismatch")]
    ProofVersion,
    #[error("v2 descriptor contains an uncommitted network, model, or PCS identity")]
    UncommittedDescriptor,
    #[error("v2 dimensions and layer banks must be nonzero powers of two and agree")]
    InvalidShape,
    #[error(
        "the production v2 profile is disabled; the reference oracle accepts only tiny profiles"
    )]
    ProductionDisabled,
    #[error("model bytes do not match the consensus manifest")]
    ModelMismatch,
    #[error("model-bank validation failed: {0}")]
    ModelBank(#[from] ModelBankError),
    #[error("reference witness has the wrong shape")]
    WitnessShape,
    #[error("reference witness challenge is not bound to the public statement")]
    ChallengeMismatch,
    #[error("matrix accumulator is incorrect")]
    Accumulator,
    #[error("signed accumulator does not have its canonical transition-field encoding")]
    EncodedAccumulator,
    #[error("transition-field witness is outside its canonical 27-bit prime range")]
    TransitionFieldRange,
    #[error("square reduction relation is incorrect")]
    SquareRelation,
    #[error("cube reduction relation is incorrect")]
    CubeRelation,
    #[error("output quotient or remainder is outside its canonical range")]
    OutputRange,
    #[error("output reduction relation is incorrect")]
    OutputRelation,
    #[error("activation is not the canonical centered encoding")]
    ActivationEncoding,
    #[error("final activation digest mismatch")]
    OutputDigest,
    #[error("work digest mismatch")]
    WorkDigest,
    #[error("work digest does not meet the block target")]
    HighHash,
    #[error("integer arithmetic exceeded the specified v2 bounds")]
    ArithmeticOverflow,
}

#[derive(Debug, Clone)]
pub struct ForgeMatrixV2Reference {
    descriptor: ForgeMatrixV2Descriptor,
    base_input: Vec<u8>,
    weights: Vec<Vec<u8>>,
}

impl ForgeMatrixV2Reference {
    pub fn from_explicit_model(
        descriptor: ForgeMatrixV2Descriptor,
        base_input: Vec<u8>,
        weights: Vec<Vec<u8>>,
    ) -> Result<Self, ForgeMatrixV2Error> {
        descriptor.validate_research()?;
        let layer_refs: Vec<&[u8]> = weights.iter().map(Vec::as_slice).collect();
        let rebuilt = build_small_model_bank(SmallModelBankFixture {
            model_version: descriptor.model.model_version,
            dimension: descriptor.model.dimension,
            batch: descriptor.model.batch,
            base_input: &base_input,
            layers: &layer_refs,
            pcs_parameter_digest: descriptor.model.pcs_parameter_digest,
            pcs_commitment_root: descriptor.model.pcs_commitment_root,
        })?;
        if rebuilt.manifest != descriptor.model {
            return Err(ForgeMatrixV2Error::ModelMismatch);
        }
        Ok(Self {
            descriptor,
            base_input,
            weights,
        })
    }

    pub fn descriptor(&self) -> ForgeMatrixV2Descriptor {
        self.descriptor
    }

    /// Binary input for the independent CUDA v2 arithmetic oracle.
    ///
    /// Format: magic, dimensions, coefficient count, challenge, raw base
    /// table, input-mask coefficients and expected initialized activation,
    /// then each raw layer, its mask coefficients, and that layer's expected
    /// canonical `v` bytes.
    pub fn gpu_fixture(
        &self,
        block: &BlockChallenge,
        nonce: u64,
    ) -> Result<Vec<u8>, ForgeMatrixV2Error> {
        let proof = self.prove_reference(block, nonce)?;
        let rows = self.descriptor.model.batch as usize;
        let width = self.descriptor.model.dimension as usize;
        let coefficient_count = 1 + rows.ilog2() as usize + width.ilog2() as usize;
        let activation_len = rows * width;
        let weight_len = width * width;
        let expected_len = 24_usize
            .checked_add(32)
            .and_then(|length| length.checked_add(activation_len))
            .and_then(|length| length.checked_add(coefficient_count))
            .and_then(|length| length.checked_add(activation_len))
            .and_then(|length| {
                length.checked_add(
                    self.weights
                        .len()
                        .checked_mul(weight_len + coefficient_count + activation_len)?,
                )
            })
            .ok_or(ForgeMatrixV2Error::ArithmeticOverflow)?;
        let mut fixture = Vec::with_capacity(expected_len);
        fixture.extend_from_slice(b"CMFDGPU2");
        fixture.extend_from_slice(&self.descriptor.model.dimension.to_le_bytes());
        fixture.extend_from_slice(&self.descriptor.model.batch.to_le_bytes());
        fixture.extend_from_slice(&self.descriptor.model.layers.to_le_bytes());
        fixture.extend_from_slice(&(coefficient_count as u32).to_le_bytes());
        fixture.extend_from_slice(&proof.challenge_digest);
        fixture.extend_from_slice(&self.base_input);
        fixture.extend_from_slice(&mask_coefficients(
            &proof.challenge_digest,
            u32::MAX,
            rows,
            width,
        ));
        fixture.extend_from_slice(&activation_bytes(&proof.initial_activation)?);
        for (layer_index, (weights, layer)) in self.weights.iter().zip(&proof.layers).enumerate() {
            fixture.extend_from_slice(weights);
            fixture.extend_from_slice(&mask_coefficients(
                &proof.challenge_digest,
                layer_index as u32,
                rows,
                width,
            ));
            fixture.extend_from_slice(&activation_bytes(&layer.output)?);
        }
        debug_assert_eq!(fixture.len(), expected_len);
        Ok(fixture)
    }

    pub fn prove_reference(
        &self,
        block: &BlockChallenge,
        nonce: u64,
    ) -> Result<ForgeMatrixV2ReferenceProof, ForgeMatrixV2Error> {
        let challenge = challenge_digest(&self.descriptor, block, nonce)?;
        let rows = self.descriptor.model.batch as usize;
        let width = self.descriptor.model.dimension as usize;
        let activation_len = rows * width;

        let input_mask = mask_coefficients(&challenge, u32::MAX, rows, width);
        let mut initial_reductions = Vec::with_capacity(activation_len);
        let mut activation = Vec::with_capacity(activation_len);
        for row in 0..rows {
            for col in 0..width {
                let index = row * width + col;
                let z = i32::from(decode_model_byte(self.base_input[index]))
                    .checked_add(mask_value(&input_mask, row, col, rows, width)?)
                    .ok_or(ForgeMatrixV2Error::ArithmeticOverflow)?;
                let reduction = reduction_witness(z)?;
                activation.push(centered_activation(reduction.output_remainder)?);
                initial_reductions.push(reduction);
            }
        }

        let initial_activation = activation.clone();
        let mut layer_witnesses = Vec::with_capacity(self.weights.len());
        for (layer_index, weights) in self.weights.iter().enumerate() {
            let mask = mask_coefficients(&challenge, layer_index as u32, rows, width);
            let mut accumulators = Vec::with_capacity(activation_len);
            let mut reductions = Vec::with_capacity(activation_len);
            let mut output = Vec::with_capacity(activation_len);
            for row in 0..rows {
                for col in 0..width {
                    let mut accumulator = 0_i64;
                    for common in 0..width {
                        let product = i64::from(activation[row * width + common])
                            * i64::from(decode_model_byte(weights[common * width + col]));
                        accumulator = accumulator
                            .checked_add(product)
                            .ok_or(ForgeMatrixV2Error::ArithmeticOverflow)?;
                    }
                    let accumulator = i32::try_from(accumulator)
                        .map_err(|_| ForgeMatrixV2Error::ArithmeticOverflow)?;
                    let z = accumulator
                        .checked_add(mask_value(&mask, row, col, rows, width)?)
                        .ok_or(ForgeMatrixV2Error::ArithmeticOverflow)?;
                    let reduction = reduction_witness(z)?;
                    accumulators.push(accumulator);
                    output.push(centered_activation(reduction.output_remainder)?);
                    reductions.push(reduction);
                }
            }
            activation = output.clone();
            layer_witnesses.push(LayerWitness {
                accumulators,
                reductions,
                output,
            });
        }

        let final_bytes = activation_bytes(&activation)?;
        let final_activation_digest = output_digest(challenge, &final_bytes);
        let work_digest = work_digest(&self.descriptor, challenge, final_activation_digest);
        Ok(ForgeMatrixV2ReferenceProof {
            nonce,
            challenge_digest: challenge,
            initial_reductions,
            initial_activation,
            layers: layer_witnesses,
            final_activation_digest,
            work_digest,
        })
    }

    pub fn verify_reference(
        &self,
        block: &BlockChallenge,
        proof: &ForgeMatrixV2ReferenceProof,
    ) -> Result<(), ForgeMatrixV2Error> {
        let challenge = challenge_digest(&self.descriptor, block, proof.nonce)?;
        if proof.challenge_digest != challenge {
            return Err(ForgeMatrixV2Error::ChallengeMismatch);
        }

        let rows = self.descriptor.model.batch as usize;
        let width = self.descriptor.model.dimension as usize;
        let activation_len = rows * width;
        if proof.initial_reductions.len() != activation_len
            || proof.initial_activation.len() != activation_len
            || proof.layers.len() != self.weights.len()
        {
            return Err(ForgeMatrixV2Error::WitnessShape);
        }

        let input_mask = mask_coefficients(&challenge, u32::MAX, rows, width);
        let mut activation = Vec::with_capacity(activation_len);
        for row in 0..rows {
            for col in 0..width {
                let index = row * width + col;
                let z = i32::from(decode_model_byte(self.base_input[index]))
                    .checked_add(mask_value(&input_mask, row, col, rows, width)?)
                    .ok_or(ForgeMatrixV2Error::ArithmeticOverflow)?;
                check_reduction(z, &proof.initial_reductions[index])?;
                let expected =
                    centered_activation(proof.initial_reductions[index].output_remainder)?;
                if proof.initial_activation[index] != expected {
                    return Err(ForgeMatrixV2Error::ActivationEncoding);
                }
                activation.push(expected);
            }
        }

        for (layer_index, (weights, layer)) in self.weights.iter().zip(&proof.layers).enumerate() {
            if layer.accumulators.len() != activation_len
                || layer.reductions.len() != activation_len
                || layer.output.len() != activation_len
            {
                return Err(ForgeMatrixV2Error::WitnessShape);
            }
            let mask = mask_coefficients(&challenge, layer_index as u32, rows, width);
            let mut next = Vec::with_capacity(activation_len);
            for row in 0..rows {
                for col in 0..width {
                    let index = row * width + col;
                    let mut expected_accumulator = 0_i64;
                    for common in 0..width {
                        expected_accumulator += i64::from(activation[row * width + common])
                            * i64::from(decode_model_byte(weights[common * width + col]));
                    }
                    if i64::from(layer.accumulators[index]) != expected_accumulator {
                        return Err(ForgeMatrixV2Error::Accumulator);
                    }
                    let z = layer.accumulators[index]
                        .checked_add(mask_value(&mask, row, col, rows, width)?)
                        .ok_or(ForgeMatrixV2Error::ArithmeticOverflow)?;
                    check_reduction(z, &layer.reductions[index])?;
                    let expected = centered_activation(layer.reductions[index].output_remainder)?;
                    if layer.output[index] != expected {
                        return Err(ForgeMatrixV2Error::ActivationEncoding);
                    }
                    next.push(expected);
                }
            }
            activation = next;
        }

        let final_bytes = activation_bytes(&activation)?;
        let expected_output = output_digest(challenge, &final_bytes);
        if proof.final_activation_digest != expected_output {
            return Err(ForgeMatrixV2Error::OutputDigest);
        }
        let expected_work = work_digest(&self.descriptor, challenge, expected_output);
        if proof.work_digest != expected_work {
            return Err(ForgeMatrixV2Error::WorkDigest);
        }
        if proof.work_digest > block.target {
            return Err(ForgeMatrixV2Error::HighHash);
        }
        Ok(())
    }
}

/// Fixed explicit test model used by Rust/CUDA differential vectors.
pub fn v2_test_reference() -> Result<ForgeMatrixV2Reference, ForgeMatrixV2Error> {
    let base = vec![0, 1, 2, 3, 247, 248, 249, 250];
    let layers: Vec<Vec<u8>> = (0..V2_TEST_LAYERS as usize)
        .map(|layer| {
            (0..(V2_TEST_DIMENSION * V2_TEST_DIMENSION) as usize)
                .map(|index| ((layer * 41 + index * 17 + 3) % 251) as u8)
                .collect()
        })
        .collect();
    let refs: Vec<&[u8]> = layers.iter().map(Vec::as_slice).collect();
    let built = build_small_model_bank(SmallModelBankFixture {
        model_version: 2,
        dimension: V2_TEST_DIMENSION,
        batch: V2_TEST_BATCH,
        base_input: &base,
        layers: &refs,
        pcs_parameter_digest: [0x91; 32],
        pcs_commitment_root: [0xa2; 32],
    })?;
    let descriptor =
        ForgeMatrixV2Descriptor::new_research([0x63; 32], 1, V2_TEST_LAYERS, built.manifest)?;
    ForgeMatrixV2Reference::from_explicit_model(descriptor, base, layers)
}

fn challenge_digest(
    descriptor: &ForgeMatrixV2Descriptor,
    block: &BlockChallenge,
    nonce: u64,
) -> Result<[u8; 32], ForgeMatrixV2Error> {
    let mut hasher = Hasher::new_derive_key(CHALLENGE_DOMAIN);
    hasher.update(&descriptor.network_id);
    hasher.update(&descriptor.algorithm_version.to_le_bytes());
    hasher.update(&descriptor.proof_version.to_le_bytes());
    hasher.update(&descriptor.banks.to_le_bytes());
    hasher.update(&descriptor.layers_per_bank.to_le_bytes());
    hasher.update(&descriptor.model.digest()?);
    hasher.update(&descriptor.model.raw_blake3_root);
    hasher.update(&descriptor.model.pcs_parameter_digest);
    hasher.update(&descriptor.model.pcs_commitment_root);
    hasher.update(&block.previous_block);
    hasher.update(&block.transaction_root);
    hasher.update(&block.height.to_le_bytes());
    hasher.update(&block.timestamp.to_le_bytes());
    hasher.update(&block.target);
    hasher.update(&nonce.to_le_bytes());
    Ok(*hasher.finalize().as_bytes())
}

fn mask_coefficients(challenge: &[u8; 32], layer: u32, rows: usize, width: usize) -> Vec<u8> {
    let count = 1 + rows.ilog2() as usize + width.ilog2() as usize;
    let mut hasher = Hasher::new_derive_key(MASK_DOMAIN);
    hasher.update(challenge);
    hasher.update(&layer.to_le_bytes());
    let mut reader = hasher.finalize_xof();
    let mut coefficients = Vec::with_capacity(count);
    let mut buffer = [0_u8; 64];
    while coefficients.len() < count {
        reader.fill(&mut buffer);
        coefficients.extend(
            buffer
                .iter()
                .copied()
                .filter(|byte| *byte <= 250)
                .take(count - coefficients.len()),
        );
    }
    coefficients
}

fn mask_value(
    coefficients: &[u8],
    row: usize,
    col: usize,
    rows: usize,
    width: usize,
) -> Result<i32, ForgeMatrixV2Error> {
    let row_bits = rows.ilog2() as usize;
    let col_bits = width.ilog2() as usize;
    if coefficients.len() != 1 + row_bits + col_bits {
        return Err(ForgeMatrixV2Error::InvalidShape);
    }
    let mut value = i32::from(coefficients[0]);
    for bit in 0..row_bits {
        if (row >> bit) & 1 == 1 {
            value += i32::from(coefficients[1 + bit]);
        }
    }
    for bit in 0..col_bits {
        if (col >> bit) & 1 == 1 {
            value += i32::from(coefficients[1 + row_bits + bit]);
        }
    }
    Ok(value)
}

fn reduction_witness(z: i32) -> Result<ReductionWitness, ForgeMatrixV2Error> {
    let modulus = i64::from(V2_TRANSITION_MODULUS);
    if i64::from(z).unsigned_abs() >= V2_TRANSITION_MODULUS.into() {
        return Err(ForgeMatrixV2Error::ArithmeticOverflow);
    }
    let encoded_z = if z >= 0 {
        z as u32
    } else {
        u32::try_from(modulus + i64::from(z)).map_err(|_| ForgeMatrixV2Error::ArithmeticOverflow)?
    };
    let square = u64::from(encoded_z) * u64::from(encoded_z);
    let square_quotient = square / u64::from(V2_TRANSITION_MODULUS);
    let square_remainder = square % u64::from(V2_TRANSITION_MODULUS);
    let cube_product = square_remainder * u64::from(encoded_z);
    let cube_quotient = cube_product / u64::from(V2_TRANSITION_MODULUS);
    let cube_remainder = cube_product % u64::from(V2_TRANSITION_MODULUS);
    let output_quotient = cube_remainder / OUTPUT_MODULUS;
    let output_remainder = cube_remainder % OUTPUT_MODULUS;
    let witness = ReductionWitness {
        z,
        encoded_z,
        square_quotient: u32::try_from(square_quotient)
            .map_err(|_| ForgeMatrixV2Error::ArithmeticOverflow)?,
        square_remainder: u32::try_from(square_remainder)
            .map_err(|_| ForgeMatrixV2Error::ArithmeticOverflow)?,
        cube_quotient: u32::try_from(cube_quotient)
            .map_err(|_| ForgeMatrixV2Error::ArithmeticOverflow)?,
        cube_remainder: u32::try_from(cube_remainder)
            .map_err(|_| ForgeMatrixV2Error::ArithmeticOverflow)?,
        output_quotient: u32::try_from(output_quotient)
            .map_err(|_| ForgeMatrixV2Error::ArithmeticOverflow)?,
        output_remainder: u16::try_from(output_remainder)
            .map_err(|_| ForgeMatrixV2Error::ArithmeticOverflow)?,
    };
    check_reduction(z, &witness)?;
    Ok(witness)
}

fn check_reduction(z: i32, witness: &ReductionWitness) -> Result<(), ForgeMatrixV2Error> {
    let modulus = u64::from(V2_TRANSITION_MODULUS);
    if witness.encoded_z >= V2_TRANSITION_MODULUS
        || witness.square_quotient >= V2_TRANSITION_MODULUS
        || witness.square_remainder >= V2_TRANSITION_MODULUS
        || witness.cube_quotient >= V2_TRANSITION_MODULUS
        || witness.cube_remainder >= V2_TRANSITION_MODULUS
    {
        return Err(ForgeMatrixV2Error::TransitionFieldRange);
    }
    if witness.output_quotient > MAX_OUTPUT_QUOTIENT || witness.output_remainder > 250 {
        return Err(ForgeMatrixV2Error::OutputRange);
    }
    let expected_encoded = if z >= 0 {
        u32::try_from(z).map_err(|_| ForgeMatrixV2Error::EncodedAccumulator)?
    } else {
        u32::try_from(i64::from(V2_TRANSITION_MODULUS) + i64::from(z))
            .map_err(|_| ForgeMatrixV2Error::EncodedAccumulator)?
    };
    if witness.z != z || witness.encoded_z != expected_encoded {
        return Err(ForgeMatrixV2Error::EncodedAccumulator);
    }
    if u64::from(witness.encoded_z) * u64::from(witness.encoded_z)
        != modulus * u64::from(witness.square_quotient) + u64::from(witness.square_remainder)
    {
        return Err(ForgeMatrixV2Error::SquareRelation);
    }
    if u64::from(witness.square_remainder) * u64::from(witness.encoded_z)
        != modulus * u64::from(witness.cube_quotient) + u64::from(witness.cube_remainder)
    {
        return Err(ForgeMatrixV2Error::CubeRelation);
    }
    if u64::from(witness.cube_remainder)
        != OUTPUT_MODULUS * u64::from(witness.output_quotient) + u64::from(witness.output_remainder)
    {
        return Err(ForgeMatrixV2Error::OutputRelation);
    }
    Ok(())
}

fn centered_activation(value: u16) -> Result<i16, ForgeMatrixV2Error> {
    if value > 250 {
        return Err(ForgeMatrixV2Error::OutputRange);
    }
    Ok(i16::try_from(value).map_err(|_| ForgeMatrixV2Error::ActivationEncoding)? - CENTER)
}

fn decode_model_byte(value: u8) -> i16 {
    i16::from(value) - CENTER
}

fn activation_bytes(values: &[i16]) -> Result<Vec<u8>, ForgeMatrixV2Error> {
    values
        .iter()
        .map(|value| {
            let encoded = *value + CENTER;
            u8::try_from(encoded)
                .ok()
                .filter(|byte| *byte <= 250)
                .ok_or(ForgeMatrixV2Error::ActivationEncoding)
        })
        .collect()
}

fn output_digest(challenge: [u8; 32], final_bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Hasher::new_derive_key(OUTPUT_DOMAIN);
    hasher.update(&challenge);
    hasher.update(&(final_bytes.len() as u64).to_le_bytes());
    hasher.update(final_bytes);
    *hasher.finalize().as_bytes()
}

fn work_digest(
    descriptor: &ForgeMatrixV2Descriptor,
    challenge: [u8; 32],
    final_activation_digest: [u8; 32],
) -> [u8; 32] {
    let mut hasher = Hasher::new_derive_key(WORK_DOMAIN);
    hasher.update(&challenge);
    hasher.update(&descriptor.model.raw_blake3_root);
    hasher.update(&descriptor.model.pcs_commitment_root);
    hasher.update(&final_activation_digest);
    *hasher.finalize().as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> ForgeMatrixV2Reference {
        v2_test_reference().unwrap()
    }

    fn block() -> BlockChallenge {
        BlockChallenge {
            previous_block: [0x11; 32],
            transaction_root: [0x22; 32],
            height: 42,
            timestamp: 1_777_777_777,
            target: [0xff; 32],
        }
    }

    #[test]
    fn exact_v2_relation_round_trips() {
        let oracle = fixture();
        let proof = oracle.prove_reference(&block(), 7).unwrap();
        oracle.verify_reference(&block(), &proof).unwrap();
        assert_eq!(
            hex::encode(proof.challenge_digest),
            "2d6700ad9876078ae116476cd57f3536fd7bfdd194340c68c16337a919d1a209"
        );
        assert_eq!(
            hex::encode(proof.final_activation_digest),
            "ca6e175a9da192bf4bbf62f89fa7bc88627c2914fb6e1b5c282801b222f815db"
        );
        assert_eq!(
            hex::encode(proof.work_digest),
            "656dec6dbf73d17cffc0f52b4e2c05afeb5d3ea701ecee350ec26dea7e598db1"
        );
        assert_eq!(proof.initial_activation.len(), 8);
        assert_eq!(proof.layers.len(), 4);
        assert!(
            proof
                .layers
                .iter()
                .flat_map(|layer| &layer.output)
                .all(|value| (-125..=125).contains(value))
        );
    }

    #[test]
    fn gpu_fixture_is_canonical_and_complete() {
        let oracle = fixture();
        let fixture = oracle.gpu_fixture(&block(), 7).unwrap();
        let coefficient_count = 1 + 1 + 2;
        let expected = 24 + 32 + 8 + coefficient_count + 8 + 4 * (16 + coefficient_count + 8);
        assert_eq!(fixture.len(), expected);
        assert_eq!(&fixture[..8], b"CMFDGPU2");
    }

    #[test]
    fn every_public_block_field_and_nonce_is_bound() {
        let oracle = fixture();
        let original = block();
        let proof = oracle.prove_reference(&original, 7).unwrap();
        let mut mutations = Vec::new();
        let mut changed = original;
        changed.previous_block[0] ^= 1;
        mutations.push(changed);
        let mut changed = original;
        changed.transaction_root[0] ^= 1;
        mutations.push(changed);
        let mut changed = original;
        changed.height += 1;
        mutations.push(changed);
        let mut changed = original;
        changed.timestamp += 1;
        mutations.push(changed);
        let mut changed = original;
        changed.target[0] ^= 1;
        mutations.push(changed);
        for changed in mutations {
            assert!(matches!(
                oracle.verify_reference(&changed, &proof),
                Err(ForgeMatrixV2Error::ChallengeMismatch)
            ));
        }
        let mut wrong_nonce = proof;
        wrong_nonce.nonce += 1;
        assert!(matches!(
            oracle.verify_reference(&original, &wrong_nonce),
            Err(ForgeMatrixV2Error::ChallengeMismatch)
        ));
    }

    #[test]
    fn skipped_or_modified_matrix_work_is_rejected() {
        let oracle = fixture();
        let original = oracle.prove_reference(&block(), 19).unwrap();
        for layer_index in [0, 2, 3] {
            let mut proof = original.clone();
            proof.layers[layer_index].accumulators[3] ^= 1;
            assert!(matches!(
                oracle.verify_reference(&block(), &proof),
                Err(ForgeMatrixV2Error::Accumulator)
            ));
        }
        let mut omitted = original;
        omitted.layers.remove(2);
        assert!(matches!(
            oracle.verify_reference(&block(), &omitted),
            Err(ForgeMatrixV2Error::WitnessShape)
        ));
    }

    #[test]
    fn transition_reductions_and_encoding_tampering_is_rejected() {
        let oracle = fixture();
        let original = oracle.prove_reference(&block(), 23).unwrap();

        let mut proof = original.clone();
        proof.layers[1].reductions[2].encoded_z += 1;
        assert!(matches!(
            oracle.verify_reference(&block(), &proof),
            Err(ForgeMatrixV2Error::EncodedAccumulator)
        ));

        let mut proof = original.clone();
        proof.layers[1].reductions[2].square_quotient += 1;
        assert!(matches!(
            oracle.verify_reference(&block(), &proof),
            Err(ForgeMatrixV2Error::SquareRelation)
        ));

        let mut proof = original.clone();
        proof.layers[1].reductions[2].cube_quotient += 1;
        assert!(matches!(
            oracle.verify_reference(&block(), &proof),
            Err(ForgeMatrixV2Error::CubeRelation)
        ));

        let mut proof = original.clone();
        proof.layers[1].reductions[2].output_quotient += 1;
        assert!(matches!(
            oracle.verify_reference(&block(), &proof),
            Err(ForgeMatrixV2Error::OutputRelation)
        ));

        let mut proof = original;
        proof.layers[1].output[2] += 1;
        assert!(matches!(
            oracle.verify_reference(&block(), &proof),
            Err(ForgeMatrixV2Error::ActivationEncoding)
        ));
    }

    #[test]
    fn transition_field_boundary_vectors_are_canonical() {
        for z in [-64_000_000, -1, 0, 1, 64_005_000] {
            let witness = reduction_witness(z).unwrap();
            check_reduction(z, &witness).unwrap();
            assert!(witness.encoded_z < V2_TRANSITION_MODULUS);
            assert!(witness.square_remainder < V2_TRANSITION_MODULUS);
            assert!(witness.cube_remainder < V2_TRANSITION_MODULUS);
            assert!(witness.output_remainder <= 250);
        }

        let mut field_out_of_range = reduction_witness(250).unwrap();
        field_out_of_range.square_remainder = V2_TRANSITION_MODULUS;
        assert!(matches!(
            check_reduction(250, &field_out_of_range),
            Err(ForgeMatrixV2Error::TransitionFieldRange)
        ));

        let mut output_out_of_range = reduction_witness(250).unwrap();
        output_out_of_range.output_remainder = 251;
        assert!(matches!(
            check_reduction(250, &output_out_of_range),
            Err(ForgeMatrixV2Error::OutputRange)
        ));

        let old_small_modulus_pair_a = reduction_witness(20_000).unwrap();
        let old_small_modulus_pair_b = reduction_witness(20_251).unwrap();
        assert_ne!(
            old_small_modulus_pair_a.output_remainder, old_small_modulus_pair_b.output_remainder,
            "values differing by 251 must not be universally interchangeable"
        );
    }

    #[test]
    fn transition_modulus_is_prime_and_cubing_is_a_permutation() {
        assert_eq!(V2_TRANSITION_MODULUS % 3, 2);
        let limit = (V2_TRANSITION_MODULUS as f64).sqrt() as u32;
        for divisor in 2..=limit {
            assert_ne!(V2_TRANSITION_MODULUS % divisor, 0);
        }
        assert!(
            u64::from(V2_TRANSITION_MODULUS)
                > u64::try_from(64_005_000_i64 - (-64_000_000_i64)).unwrap()
        );
    }

    #[test]
    fn reference_constructor_cannot_activate_production_shape() {
        let model = ModelBankManifest {
            model_version: 2,
            dimension: PRODUCTION_V2_DIMENSION,
            batch: PRODUCTION_V2_BATCH,
            layers: PRODUCTION_V2_LAYERS,
            base_input_bytes: u64::from(PRODUCTION_V2_BATCH * PRODUCTION_V2_DIMENSION),
            bytes_per_layer: u64::from(PRODUCTION_V2_DIMENSION)
                * u64::from(PRODUCTION_V2_DIMENSION),
            payload_bytes: u64::from(PRODUCTION_V2_BATCH * PRODUCTION_V2_DIMENSION)
                + u64::from(PRODUCTION_V2_LAYERS)
                    * u64::from(PRODUCTION_V2_DIMENSION)
                    * u64::from(PRODUCTION_V2_DIMENSION),
            raw_blake3_root: [1; 32],
            layer_roots_aggregate: [2; 32],
            pcs_parameter_digest: [3; 32],
            pcs_commitment_root: [4; 32],
        };
        assert!(matches!(
            ForgeMatrixV2Descriptor::new_research(
                [5; 32],
                PRODUCTION_V2_BANKS,
                PRODUCTION_V2_LAYERS_PER_BANK,
                model,
            ),
            Err(ForgeMatrixV2Error::ProductionDisabled)
        ));
    }
}
