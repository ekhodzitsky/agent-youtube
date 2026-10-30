//! Original AAC-LC / HE-AAC engine (ISO/IEC 14496-3 / 13818-7).
//!
//! Product entry is [`decode::StreamDecoder`].

pub mod adts;
pub(crate) mod adts_crc;
pub(crate) mod adts_rdb;
pub mod asc;
pub mod bits;
pub(crate) mod cce;
pub(crate) mod channel_map;
pub mod crc;
pub mod decode;
pub(crate) mod decode_cpe;
pub(crate) mod decode_finish;
pub(crate) mod decode_ld;
pub(crate) mod det_math;
pub(crate) mod enc_alloc;
pub(crate) mod enc_frame;
pub(crate) mod enc_group;
pub(crate) mod enc_huff;
pub(crate) mod enc_is;
pub(crate) mod enc_ld;
pub(crate) mod enc_ms;
pub(crate) mod enc_pad;
pub(crate) mod enc_pns;
pub(crate) mod enc_psy;
pub(crate) mod enc_quant;
// HE v1 encode (TASK-86..89); public options/signalling are TASK-90.
pub(crate) mod enc_he;
pub(crate) mod enc_he_ps;
pub(crate) mod enc_mc;
pub(crate) mod enc_ps_bits;
pub(crate) mod enc_ps_est;
pub(crate) mod enc_sbr_bits;
pub(crate) mod enc_sbr_est;
pub(crate) mod enc_sbr_header;
pub(crate) mod enc_sbr_prep;
pub(crate) mod enc_sbr_qmf;
pub(crate) mod enc_section;
pub(crate) mod enc_short;
pub(crate) mod enc_tns;
pub(crate) mod error;
pub mod extension_payload;
pub(crate) mod fb_pool;
pub mod filterbank;
pub(crate) mod filterbank_simd;
pub(crate) mod heap;
pub mod huff;
pub mod huff_esc;
pub mod huff_pair;
pub mod huff_quad;
pub mod ics;
pub mod ics_body;
pub mod imdct;
pub mod latm;
pub(crate) mod latm_write;
pub(crate) mod ltp;
pub(crate) mod mdct;
pub mod pns;
pub(crate) mod pred;
pub(crate) mod prof;
pub mod ps_data;
pub mod ps_decoder;
pub mod ps_decorr;
pub mod ps_huffman;
pub mod ps_hybrid;
pub mod ps_map;
pub mod ps_stereo;
pub mod raw_data_block;
pub(crate) mod sbr_attach;
pub mod sbr_decoder;
pub mod sbr_dequant;
pub mod sbr_element;
pub mod sbr_env_adjust;
pub mod sbr_envelope;
pub mod sbr_extension;
pub mod sbr_freq_bands;
pub mod sbr_grid;
pub mod sbr_header;
pub mod sbr_hf_gen;
pub mod sbr_huffman;
pub mod sbr_limiter;
pub mod sbr_noise_table;
pub mod sbr_qmf;
pub mod sbr_reconstruct;
pub mod sbr_time_grid;
pub mod section;
pub mod sf;
pub mod sf_tab;
pub mod skip;
pub mod spectrum;
pub(crate) mod spectrum_scaled;
pub(crate) mod ssr;
pub(crate) mod ssr_pqf;
pub mod stereo;
pub mod swb;
pub mod tns;
pub(crate) mod tools;

pub(crate) use error::Error;
pub use error::Result;

#[cfg(test)]
#[path = "huff_tests.rs"]
mod huff_tests;

#[cfg(test)]
#[path = "ics_tests.rs"]
mod ics_tests;

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tools_tests;

#[cfg(test)]
#[path = "profile_tests.rs"]
mod profile_tests;

#[cfg(test)]
#[path = "channel_map_tests.rs"]
mod channel_map_tests;

#[cfg(test)]
#[path = "channel_map_71_tests.rs"]
mod channel_map_71_tests;

#[cfg(test)]
#[path = "skip_tests.rs"]
mod skip_tests;

#[cfg(test)]
#[path = "cce_tests.rs"]
mod cce_tests;

#[cfg(test)]
#[path = "he_config_tests.rs"]
mod he_config_tests;

#[cfg(test)]
#[path = "sbr_attach_tests.rs"]
mod sbr_attach_tests;

#[cfg(test)]
#[path = "sbr_lifecycle_tests.rs"]
mod sbr_lifecycle_tests;

#[cfg(test)]
#[path = "sbr_workspace_tests.rs"]
mod sbr_workspace_tests;

#[cfg(test)]
#[path = "ps_workspace_tests.rs"]
mod ps_workspace_tests;

#[cfg(test)]
#[path = "fb_pool_tests.rs"]
mod fb_pool_tests;

#[cfg(test)]
#[path = "golden_tests.rs"]
mod golden_tests;

#[cfg(test)]
#[path = "extension_payload_tests.rs"]
mod extension_payload_tests;

#[cfg(test)]
#[path = "pcm_host_probe.rs"]
mod pcm_host_probe;
