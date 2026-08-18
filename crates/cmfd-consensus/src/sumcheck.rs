//! Test-size matrix-multiplication sumcheck.
//!
//! This is an executable transcript skeleton for ForgeMatrix v2. It checks a
//! matrix product with the standard degree-two sumcheck, but the verifier is
//! intentionally given all three matrices and recomputes their multilinear
//! openings. It is therefore **not** the production succinct proof. Replacing
//! those full-table openings with the consensus-pinned transparent PCS is an
//! explicit mainnet gate.

use blake3::Hasher;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const GOLDILOCKS_MODULUS: u64 = 0xffff_ffff_0000_0001;
pub const MAX_TOY_MATRIX_ELEMENTS: usize = 4096;
pub const TOY_SUMCHECK_RAW_CHALLENGE_BITS: u32 = 64;

const TRANSCRIPT_DOMAIN: &str = "CMFD/FORGEMATRIX/SUMCHECK/TOY/V2";
const MATRIX_COMMITMENT_DOMAIN: &str = "CMFD/FORGEMATRIX/MATRIX/TOY/V2";
const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixProductSumcheckProof {
    pub protocol_version: u32,
    pub a_commitment: [u8; 32],
    pub b_commitment: [u8; 32],
    pub c_commitment: [u8; 32],
    pub c_evaluation: u64,
    pub rounds: Vec<[u64; 3]>,
    pub a_final: u64,
    pub b_final: u64,
}

impl MatrixProductSumcheckProof {
    pub fn canonical_size(&self) -> usize {
        4 + 3 * 32 + 8 + self.rounds.len() * 3 * 8 + 2 * 8
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SumcheckError {
    #[error("matrix dimensions must be nonzero powers of two")]
    InvalidDimensions,
    #[error("matrix input length does not match its dimensions")]
    InvalidLength,
    #[error("toy sumcheck is capped at 4096 elements per matrix")]
    ProductionDisabled,
    #[error("matrix commitment does not match the verifier-owned table")]
    Commitment,
    #[error("proof protocol version mismatch")]
    ProtocolVersion,
    #[error("proof contains a noncanonical Goldilocks field element")]
    NonCanonicalField,
    #[error("proof has the wrong number of sumcheck rounds")]
    RoundCount,
    #[error("sumcheck round does not preserve the claimed sum")]
    RoundClaim,
    #[error("terminal product claim is invalid")]
    TerminalClaim,
    #[error("claimed multilinear opening does not match the verifier-owned table")]
    Opening,
    #[error("matrix multiplication overflowed the reference i64 representation")]
    ArithmeticOverflow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Goldilocks(u64);

impl Goldilocks {
    const ZERO: Self = Self(0);
    const ONE: Self = Self(1);
    const INV_TWO: Self = Self(0x7fff_ffff_8000_0001);

    fn canonical(value: u64) -> Result<Self, SumcheckError> {
        if value < GOLDILOCKS_MODULUS {
            Ok(Self(value))
        } else {
            Err(SumcheckError::NonCanonicalField)
        }
    }

    fn from_signed(value: i64) -> Self {
        if value >= 0 {
            Self((value as u64) % GOLDILOCKS_MODULUS)
        } else {
            let magnitude = value.unsigned_abs() % GOLDILOCKS_MODULUS;
            if magnitude == 0 {
                Self::ZERO
            } else {
                Self(GOLDILOCKS_MODULUS - magnitude)
            }
        }
    }

    fn add(self, rhs: Self) -> Self {
        Self(((u128::from(self.0) + u128::from(rhs.0)) % u128::from(GOLDILOCKS_MODULUS)) as u64)
    }

    fn sub(self, rhs: Self) -> Self {
        if self.0 >= rhs.0 {
            Self(self.0 - rhs.0)
        } else {
            Self(GOLDILOCKS_MODULUS - (rhs.0 - self.0))
        }
    }

    fn mul(self, rhs: Self) -> Self {
        Self(((u128::from(self.0) * u128::from(rhs.0)) % u128::from(GOLDILOCKS_MODULUS)) as u64)
    }

    fn double(self) -> Self {
        self.add(self)
    }

    fn encode(self) -> [u8; 8] {
        self.0.to_le_bytes()
    }
}

struct Transcript {
    hasher: Hasher,
    challenge_counter: u64,
}

impl Transcript {
    fn new(binding: &[u8], rows: usize, inner: usize, cols: usize) -> Self {
        let mut transcript = Self {
            hasher: Hasher::new_derive_key(TRANSCRIPT_DOMAIN),
            challenge_counter: 0,
        };
        transcript.absorb(b"protocol-version", &PROTOCOL_VERSION.to_le_bytes());
        transcript.absorb(b"public-binding", binding);
        transcript.absorb(b"rows", &(rows as u64).to_le_bytes());
        transcript.absorb(b"inner", &(inner as u64).to_le_bytes());
        transcript.absorb(b"cols", &(cols as u64).to_le_bytes());
        transcript
    }

    fn absorb(&mut self, label: &[u8], value: &[u8]) {
        self.hasher.update(&(label.len() as u64).to_le_bytes());
        self.hasher.update(label);
        self.hasher.update(&(value.len() as u64).to_le_bytes());
        self.hasher.update(value);
    }

    fn challenge(&mut self, label: &[u8]) -> Goldilocks {
        let mut attempt = 0_u64;
        loop {
            let mut candidate_hasher = self.hasher.clone();
            candidate_hasher.update(&(label.len() as u64).to_le_bytes());
            candidate_hasher.update(label);
            candidate_hasher.update(&self.challenge_counter.to_le_bytes());
            candidate_hasher.update(&attempt.to_le_bytes());
            let digest = candidate_hasher.finalize();
            let mut bytes = [0_u8; 8];
            bytes.copy_from_slice(&digest.as_bytes()[..8]);
            let candidate = u64::from_le_bytes(bytes);
            if candidate < GOLDILOCKS_MODULUS {
                let field = Goldilocks(candidate);
                self.challenge_counter += 1;
                self.absorb(b"derived-challenge", &field.encode());
                return field;
            }
            attempt += 1;
        }
    }
}

pub fn reference_matrix_product(
    a: &[i64],
    b: &[i64],
    rows: usize,
    inner: usize,
    cols: usize,
) -> Result<Vec<i64>, SumcheckError> {
    validate_shape(a, b, None, rows, inner, cols)?;
    let mut product = Vec::with_capacity(rows * cols);
    for row in 0..rows {
        for col in 0..cols {
            let mut accumulator = 0_i128;
            for common in 0..inner {
                accumulator +=
                    i128::from(a[row * inner + common]) * i128::from(b[common * cols + col]);
            }
            product
                .push(i64::try_from(accumulator).map_err(|_| SumcheckError::ArithmeticOverflow)?);
        }
    }
    Ok(product)
}

#[allow(clippy::too_many_arguments)]
pub fn prove_toy_matrix_product(
    binding: &[u8],
    a: &[i64],
    b: &[i64],
    c: &[i64],
    rows: usize,
    inner: usize,
    cols: usize,
) -> Result<MatrixProductSumcheckProof, SumcheckError> {
    validate_shape(a, b, Some(c), rows, inner, cols)?;
    let a_values = field_values(a);
    let b_values = field_values(b);
    let c_values = field_values(c);
    let a_commitment = matrix_commitment(b"A", rows, inner, &a_values);
    let b_commitment = matrix_commitment(b"B", inner, cols, &b_values);
    let c_commitment = matrix_commitment(b"C", rows, cols, &c_values);

    let mut transcript = Transcript::new(binding, rows, inner, cols);
    transcript.absorb(b"A-commitment", &a_commitment);
    transcript.absorb(b"B-commitment", &b_commitment);
    transcript.absorb(b"C-commitment", &c_commitment);
    let row_point = challenge_vector(&mut transcript, b"row-point", rows.ilog2() as usize);
    let col_point = challenge_vector(&mut transcript, b"col-point", cols.ilog2() as usize);

    let mut c_point = col_point.clone();
    c_point.extend_from_slice(&row_point);
    let c_evaluation = evaluate_mle(&c_values, &c_point);
    transcript.absorb(b"C-evaluation", &c_evaluation.encode());

    let row_weights = equality_weights(&row_point);
    let col_weights = equality_weights(&col_point);
    let mut a_partial = Vec::with_capacity(inner);
    let mut b_partial = Vec::with_capacity(inner);
    for common in 0..inner {
        let mut a_value = Goldilocks::ZERO;
        for row in 0..rows {
            a_value = a_value.add(a_values[row * inner + common].mul(row_weights[row]));
        }
        a_partial.push(a_value);

        let mut b_value = Goldilocks::ZERO;
        for col in 0..cols {
            b_value = b_value.add(b_values[common * cols + col].mul(col_weights[col]));
        }
        b_partial.push(b_value);
    }

    let mut rounds = Vec::with_capacity(inner.ilog2() as usize);
    for round_index in 0..inner.ilog2() as usize {
        let message = quadratic_round(&a_partial, &b_partial);
        let encoded = encode_round(message);
        transcript.absorb(b"round-index", &(round_index as u64).to_le_bytes());
        transcript.absorb(b"round-message", &encoded);
        let challenge = transcript.challenge(b"sumcheck-round");
        a_partial = fold(&a_partial, challenge);
        b_partial = fold(&b_partial, challenge);
        rounds.push([message[0].0, message[1].0, message[2].0]);
    }

    Ok(MatrixProductSumcheckProof {
        protocol_version: PROTOCOL_VERSION,
        a_commitment,
        b_commitment,
        c_commitment,
        c_evaluation: c_evaluation.0,
        rounds,
        a_final: a_partial[0].0,
        b_final: b_partial[0].0,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn verify_toy_matrix_product(
    binding: &[u8],
    a: &[i64],
    b: &[i64],
    c: &[i64],
    rows: usize,
    inner: usize,
    cols: usize,
    proof: &MatrixProductSumcheckProof,
) -> Result<(), SumcheckError> {
    validate_shape(a, b, Some(c), rows, inner, cols)?;
    if proof.protocol_version != PROTOCOL_VERSION {
        return Err(SumcheckError::ProtocolVersion);
    }
    if proof.rounds.len() != inner.ilog2() as usize {
        return Err(SumcheckError::RoundCount);
    }

    let a_values = field_values(a);
    let b_values = field_values(b);
    let c_values = field_values(c);
    let a_commitment = matrix_commitment(b"A", rows, inner, &a_values);
    let b_commitment = matrix_commitment(b"B", inner, cols, &b_values);
    let c_commitment = matrix_commitment(b"C", rows, cols, &c_values);
    if proof.a_commitment != a_commitment
        || proof.b_commitment != b_commitment
        || proof.c_commitment != c_commitment
    {
        return Err(SumcheckError::Commitment);
    }

    let mut transcript = Transcript::new(binding, rows, inner, cols);
    transcript.absorb(b"A-commitment", &a_commitment);
    transcript.absorb(b"B-commitment", &b_commitment);
    transcript.absorb(b"C-commitment", &c_commitment);
    let row_point = challenge_vector(&mut transcript, b"row-point", rows.ilog2() as usize);
    let col_point = challenge_vector(&mut transcript, b"col-point", cols.ilog2() as usize);
    let mut c_point = col_point.clone();
    c_point.extend_from_slice(&row_point);
    let expected_c = evaluate_mle(&c_values, &c_point);
    let c_evaluation = Goldilocks::canonical(proof.c_evaluation)?;
    if c_evaluation != expected_c {
        return Err(SumcheckError::Opening);
    }
    transcript.absorb(b"C-evaluation", &c_evaluation.encode());

    let mut claim = c_evaluation;
    let mut common_point = Vec::with_capacity(proof.rounds.len());
    for (round_index, encoded_round) in proof.rounds.iter().enumerate() {
        let message = [
            Goldilocks::canonical(encoded_round[0])?,
            Goldilocks::canonical(encoded_round[1])?,
            Goldilocks::canonical(encoded_round[2])?,
        ];
        if message[0].add(message[1]) != claim {
            return Err(SumcheckError::RoundClaim);
        }
        transcript.absorb(b"round-index", &(round_index as u64).to_le_bytes());
        transcript.absorb(b"round-message", &encode_round(message));
        let challenge = transcript.challenge(b"sumcheck-round");
        claim = evaluate_quadratic(message, challenge);
        common_point.push(challenge);
    }

    let a_final = Goldilocks::canonical(proof.a_final)?;
    let b_final = Goldilocks::canonical(proof.b_final)?;
    if claim != a_final.mul(b_final) {
        return Err(SumcheckError::TerminalClaim);
    }
    let mut a_point = common_point.clone();
    a_point.extend_from_slice(&row_point);
    let mut b_point = col_point;
    b_point.extend_from_slice(&common_point);
    if a_final != evaluate_mle(&a_values, &a_point) || b_final != evaluate_mle(&b_values, &b_point)
    {
        return Err(SumcheckError::Opening);
    }
    Ok(())
}

fn validate_shape(
    a: &[i64],
    b: &[i64],
    c: Option<&[i64]>,
    rows: usize,
    inner: usize,
    cols: usize,
) -> Result<(), SumcheckError> {
    if rows == 0
        || inner == 0
        || cols == 0
        || !rows.is_power_of_two()
        || !inner.is_power_of_two()
        || !cols.is_power_of_two()
    {
        return Err(SumcheckError::InvalidDimensions);
    }
    let a_len = rows
        .checked_mul(inner)
        .ok_or(SumcheckError::InvalidLength)?;
    let b_len = inner
        .checked_mul(cols)
        .ok_or(SumcheckError::InvalidLength)?;
    let c_len = rows.checked_mul(cols).ok_or(SumcheckError::InvalidLength)?;
    if a.len() != a_len || b.len() != b_len || c.is_some_and(|values| values.len() != c_len) {
        return Err(SumcheckError::InvalidLength);
    }
    if a_len > MAX_TOY_MATRIX_ELEMENTS
        || b_len > MAX_TOY_MATRIX_ELEMENTS
        || c_len > MAX_TOY_MATRIX_ELEMENTS
    {
        return Err(SumcheckError::ProductionDisabled);
    }
    Ok(())
}

fn field_values(values: &[i64]) -> Vec<Goldilocks> {
    values
        .iter()
        .copied()
        .map(Goldilocks::from_signed)
        .collect()
}

fn matrix_commitment(label: &[u8], rows: usize, cols: usize, values: &[Goldilocks]) -> [u8; 32] {
    let mut hasher = Hasher::new_derive_key(MATRIX_COMMITMENT_DOMAIN);
    hasher.update(&(label.len() as u64).to_le_bytes());
    hasher.update(label);
    hasher.update(&(rows as u64).to_le_bytes());
    hasher.update(&(cols as u64).to_le_bytes());
    for value in values {
        hasher.update(&value.encode());
    }
    *hasher.finalize().as_bytes()
}

fn challenge_vector(transcript: &mut Transcript, label: &[u8], count: usize) -> Vec<Goldilocks> {
    (0..count)
        .map(|index| {
            transcript.absorb(b"point-index", &(index as u64).to_le_bytes());
            transcript.challenge(label)
        })
        .collect()
}

fn equality_weights(point: &[Goldilocks]) -> Vec<Goldilocks> {
    let mut weights = vec![Goldilocks::ONE];
    for challenge in point {
        let previous = weights;
        let half = previous.len();
        weights = vec![Goldilocks::ZERO; half * 2];
        for (index, value) in previous.into_iter().enumerate() {
            weights[index] = value.mul(Goldilocks::ONE.sub(*challenge));
            weights[index + half] = value.mul(*challenge);
        }
    }
    weights
}

fn evaluate_mle(evaluations: &[Goldilocks], point: &[Goldilocks]) -> Goldilocks {
    debug_assert_eq!(evaluations.len(), 1_usize << point.len());
    let mut table = evaluations.to_vec();
    for challenge in point {
        table = fold(&table, *challenge);
    }
    table[0]
}

fn fold(table: &[Goldilocks], challenge: Goldilocks) -> Vec<Goldilocks> {
    debug_assert_eq!(table.len() % 2, 0);
    table
        .chunks_exact(2)
        .map(|pair| pair[0].add(pair[1].sub(pair[0]).mul(challenge)))
        .collect()
}

fn quadratic_round(a: &[Goldilocks], b: &[Goldilocks]) -> [Goldilocks; 3] {
    debug_assert_eq!(a.len(), b.len());
    debug_assert_eq!(a.len() % 2, 0);
    let mut values = [Goldilocks::ZERO; 3];
    for (a_pair, b_pair) in a.chunks_exact(2).zip(b.chunks_exact(2)) {
        let a_at_two = a_pair[1].double().sub(a_pair[0]);
        let b_at_two = b_pair[1].double().sub(b_pair[0]);
        values[0] = values[0].add(a_pair[0].mul(b_pair[0]));
        values[1] = values[1].add(a_pair[1].mul(b_pair[1]));
        values[2] = values[2].add(a_at_two.mul(b_at_two));
    }
    values
}

fn evaluate_quadratic(values: [Goldilocks; 3], point: Goldilocks) -> Goldilocks {
    let first_difference = values[1].sub(values[0]);
    let second_difference = values[2].sub(values[1].double()).add(values[0]);
    values[0].add(first_difference.mul(point)).add(
        second_difference
            .mul(point)
            .mul(point.sub(Goldilocks::ONE))
            .mul(Goldilocks::INV_TWO),
    )
}

fn encode_round(values: [Goldilocks; 3]) -> [u8; 24] {
    let mut encoded = [0_u8; 24];
    encoded[..8].copy_from_slice(&values[0].encode());
    encoded[8..16].copy_from_slice(&values[1].encode());
    encoded[16..].copy_from_slice(&values[2].encode());
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matrices() -> (Vec<i64>, Vec<i64>, Vec<i64>) {
        let a = vec![2, -1, 3, 4, 5, 0, -2, 1];
        let b = vec![1, 2, 3, 4, -1, 5, 0, 2, 3, -2, 4, 1, 2, 1, -3, 6];
        let c = reference_matrix_product(&a, &b, 2, 4, 4).unwrap();
        (a, b, c)
    }

    #[test]
    fn matrix_sumcheck_round_trips() {
        let (a, b, c) = matrices();
        let proof =
            prove_toy_matrix_product(b"bound block and model", &a, &b, &c, 2, 4, 4).unwrap();
        verify_toy_matrix_product(b"bound block and model", &a, &b, &c, 2, 4, 4, &proof).unwrap();
        assert_eq!(
            hex::encode(proof.a_commitment),
            "8b834c399203c6a9ac8499b70f290350839bbfd5c8e3c91803c82e959ec25941"
        );
        assert_eq!(proof.c_evaluation, 7_809_317_033_711_287_741);
        assert_eq!(
            proof.rounds[0],
            [
                16_024_862_633_393_450_704,
                10_231_198_469_732_421_358,
                6_710_379_556_497_368_171,
            ]
        );
        assert_eq!(proof.rounds.len(), 2);
        assert_eq!(proof.canonical_size(), 172);
    }

    #[test]
    fn transcript_cannot_be_replayed_on_another_statement() {
        let (a, b, c) = matrices();
        let proof = prove_toy_matrix_product(b"block A", &a, &b, &c, 2, 4, 4).unwrap();
        assert!(verify_toy_matrix_product(b"block B", &a, &b, &c, 2, 4, 4, &proof).is_err());
    }

    #[test]
    fn wrong_product_and_round_messages_are_rejected() {
        let (a, b, c) = matrices();
        let original = prove_toy_matrix_product(b"statement", &a, &b, &c, 2, 4, 4).unwrap();

        let mut wrong_c = c.clone();
        wrong_c[3] += 1;
        assert!(
            verify_toy_matrix_product(b"statement", &a, &b, &wrong_c, 2, 4, 4, &original).is_err()
        );

        for round in 0..original.rounds.len() {
            for value in 0..3 {
                let mut tampered = original.clone();
                tampered.rounds[round][value] =
                    (tampered.rounds[round][value] + 1) % GOLDILOCKS_MODULUS;
                assert!(
                    verify_toy_matrix_product(b"statement", &a, &b, &c, 2, 4, 4, &tampered,)
                        .is_err()
                );
            }
        }
    }

    #[test]
    fn commitments_openings_and_round_count_are_bound() {
        let (a, b, c) = matrices();
        let original = prove_toy_matrix_product(b"statement", &a, &b, &c, 2, 4, 4).unwrap();

        let mut commitment = original.clone();
        commitment.b_commitment[0] ^= 1;
        assert_eq!(
            verify_toy_matrix_product(b"statement", &a, &b, &c, 2, 4, 4, &commitment),
            Err(SumcheckError::Commitment)
        );

        let mut opening = original.clone();
        opening.a_final = (opening.a_final + 1) % GOLDILOCKS_MODULUS;
        assert!(verify_toy_matrix_product(b"statement", &a, &b, &c, 2, 4, 4, &opening).is_err());

        let mut short = original;
        short.rounds.pop();
        assert_eq!(
            verify_toy_matrix_product(b"statement", &a, &b, &c, 2, 4, 4, &short),
            Err(SumcheckError::RoundCount)
        );
    }

    #[test]
    fn noncanonical_fields_and_production_sizes_are_rejected() {
        let (a, b, c) = matrices();
        let mut proof = prove_toy_matrix_product(b"statement", &a, &b, &c, 2, 4, 4).unwrap();
        proof.c_evaluation = GOLDILOCKS_MODULUS;
        assert_eq!(
            verify_toy_matrix_product(b"statement", &a, &b, &c, 2, 4, 4, &proof),
            Err(SumcheckError::NonCanonicalField)
        );

        let too_large_a = vec![0_i64; 8192];
        let too_large_b = vec![0_i64; 8192];
        let too_large_c = vec![0_i64; 4096];
        assert_eq!(
            prove_toy_matrix_product(
                b"statement",
                &too_large_a,
                &too_large_b,
                &too_large_c,
                64,
                128,
                64
            ),
            Err(SumcheckError::ProductionDisabled)
        );
    }
}
