// fcall 0x860 - 0x8DF (128 fcalls)

pub const FCALL_SECP256K1_FP_INV_ID: u16 = 1;
pub const FCALL_SECP256K1_FN_INV_ID: u16 = 2;
pub const FCALL_SECP256K1_FP_SQRT_ID: u16 = 3;
pub const FCALL_MSB_POS_256_ID: u16 = 4;
pub const FCALL_SECP256K1_MSM_EDGES_ID: u16 = 5;

mod msb_pos_256;
mod secp256k1_fn_inv;
mod secp256k1_fp_inv;
mod secp256k1_fp_sqrt;
pub use msb_pos_256::*;
pub use secp256k1_fn_inv::*;
pub use secp256k1_fp_inv::*;
pub use secp256k1_fp_sqrt::*;
