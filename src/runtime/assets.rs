//! Immutable download identities; trust comes from complete SHA-256 verification.
use crate::pipeline::ModelVariant;
#[derive(Clone, Copy)]
pub(super) struct Asset {
    pub name: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha: &'static str,
}
pub(super) const ARCHIVE: Asset = Asset {
    name: "gigastt-2.22.0.tar.gz",
    url: "https://github.com/ekhodzitsky/gigastt/releases/download/v2.22.0/gigastt-2.22.0-x86_64-unknown-linux-gnu.tar.gz",
    bytes: 21_416_334,
    sha: "cd790191a600df48aa5c56e124953322af89df4bbd60d2c980436b0597c46194",
};
pub(super) fn models(variant: ModelVariant) -> &'static [Asset] {
    match variant {
        ModelVariant::E2eRnnt => &[
            Asset {
                name: "v3_e2e_rnnt_encoder_int8.onnx",
                bytes: 319_665_270,
                sha: "16805903b102d8b27bb0253de044f25e33a8a5bf07fce0c4da4ac1a5a7544091",
                url: "https://github.com/ekhodzitsky/gigastt/releases/download/models-v3-2026-10-09/v3_e2e_rnnt_encoder_int8.onnx",
            },
            Asset {
                name: "v3_e2e_rnnt_decoder.onnx",
                bytes: 4_599_910,
                sha: "7b0a16d67fd2cb37061decc93c69e364a9ab27afee3c57495d55b1c974cf7231",
                url: "https://github.com/ekhodzitsky/gigastt/releases/download/models-v3-2026-10-09/v3_e2e_rnnt_decoder.onnx",
            },
            Asset {
                name: "v3_e2e_rnnt_joint.onnx",
                bytes: 2_712_896,
                sha: "602ff7017a93311aad34df1437c8d7f49911353c13d6eae7a6ee7b041339465c",
                url: "https://github.com/ekhodzitsky/gigastt/releases/download/models-v3-2026-10-09/v3_e2e_rnnt_joint.onnx",
            },
            Asset {
                name: "v3_e2e_rnnt_vocab.txt",
                bytes: 13_354,
                sha: "39abae20e692998290c574e606f11a9edef2902a1995463fcff63d1490cf22b7",
                url: "https://github.com/ekhodzitsky/gigastt/releases/download/models-v3-2026-10-09/v3_e2e_rnnt_vocab.txt",
            },
        ],
        ModelVariant::MlCtc => &[
            Asset {
                name: "multilingual_ctc.int8.onnx",
                bytes: 224_762_204,
                sha: "e08e27ae5669b39f0c378fae101bbbb9a80505f74f9b66719c309bf5b894a480",
                url: "https://huggingface.co/istupakov/gigaam-multilingual-ctc-onnx/resolve/458860e1983aef670dd9795fb6af603c82767d5d/multilingual_ctc.int8.onnx",
            },
            Asset {
                name: "multilingual_vocab.txt",
                bytes: 393,
                sha: "4d130287892e1099fedfb3f93c4b4cf8a263151158801680b28977d1be4133f4",
                url: "https://huggingface.co/istupakov/gigaam-multilingual-ctc-onnx/resolve/458860e1983aef670dd9795fb6af603c82767d5d/multilingual_vocab.txt",
            },
        ],
        ModelVariant::MlCtcLarge => &[
            Asset {
                name: "multilingual_large_ctc.int8.onnx",
                bytes: 591_644_782,
                sha: "b2ad9c38fc04197ba758105d33f7404fd13d977958722e0f49e3f3e22521f1c6",
                url: "https://huggingface.co/istupakov/gigaam-multilingual-large-ctc-onnx/resolve/07665ab5e54371dd1ac7b8b10f06478003723573/multilingual_large_ctc.int8.onnx",
            },
            Asset {
                name: "multilingual_vocab.txt",
                bytes: 393,
                sha: "4d130287892e1099fedfb3f93c4b4cf8a263151158801680b28977d1be4133f4",
                url: "https://huggingface.co/istupakov/gigaam-multilingual-large-ctc-onnx/resolve/07665ab5e54371dd1ac7b8b10f06478003723573/multilingual_vocab.txt",
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_matches_the_files_required_by_recognition() {
        for variant in [
            ModelVariant::E2eRnnt,
            ModelVariant::MlCtc,
            ModelVariant::MlCtcLarge,
        ] {
            let names: Vec<_> = models(variant).iter().map(|a| a.name).collect();
            assert_eq!(names, variant.files());
            for asset in models(variant) {
                super::super::transfer::validate_url(asset.url).unwrap();
                assert_eq!(asset.sha.len(), 64);
                assert!(asset.bytes > 0);
            }
        }
    }
}
