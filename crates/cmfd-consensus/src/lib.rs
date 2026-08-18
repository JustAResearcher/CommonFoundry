pub mod chain;
pub mod difficulty;
pub mod economics;
pub mod forgematrix;
pub mod forgematrix_v2;
pub mod model_bank;
pub mod sumcheck;

pub use chain::{
    Block, ChainError, ChainState, Coinbase, InputWitness, OutPoint, OutputLock,
    RewardDestinations, Transaction, TxInput, TxOutput, UtxoSet, merkle_root,
};
pub use difficulty::{DifficultyError, HeaderWork, next_work_target};
pub use economics::{
    Allocation, COIN, CoinbaseClaim, DEFAULT_MONETARY_POLICY, EconomicsError, MonetaryPolicy,
};
pub use forgematrix::{
    BlockChallenge, ForgeMatrixError, ForgeMatrixProfile, ForgeMatrixProof, ForgeMatrixVerifier,
    ProfileMetrics, TEST_PROFILE,
};
pub use forgematrix_v2::{
    FORGEMATRIX_V2_ALGORITHM_VERSION, FORGEMATRIX_V2_PROOF_VERSION, ForgeMatrixV2Descriptor,
    ForgeMatrixV2Error, ForgeMatrixV2Reference, ForgeMatrixV2ReferenceProof, LayerWitness,
    PRODUCTION_V2_BANKS, PRODUCTION_V2_BATCH, PRODUCTION_V2_DIMENSION, PRODUCTION_V2_LAYERS,
    PRODUCTION_V2_LAYERS_PER_BANK, ReductionWitness, V2_REFERENCE_MAX_BATCH,
    V2_REFERENCE_MAX_DIMENSION, V2_REFERENCE_MAX_LAYERS, V2_TEST_BATCH, V2_TEST_DIMENSION,
    V2_TEST_LAYERS, V2_TRANSITION_MODULUS, v2_test_reference,
};
pub use model_bank::{
    BuiltModelBankFixture, MAX_MODEL_BYTE, MAX_SMALL_FIXTURE_PAYLOAD_BYTES,
    MODEL_BANK_FORMAT_VERSION, MODEL_BANK_HEADER_BYTES, MODEL_BANK_MAGIC, ModelBankError,
    ModelBankManifest, SmallModelBankFixture, build_small_model_bank, verify_model_bank,
};
pub use sumcheck::{
    GOLDILOCKS_MODULUS, MAX_TOY_MATRIX_ELEMENTS, MatrixProductSumcheckProof, SumcheckError,
    TOY_SUMCHECK_RAW_CHALLENGE_BITS, prove_toy_matrix_product, reference_matrix_product,
    verify_toy_matrix_product,
};
