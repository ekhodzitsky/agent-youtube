//! In-place [`super::LcEncoder`] reset: keep windows/psy, drop signal.

use super::LcEncoder;
use crate::engine::enc_group::Grouping;
use crate::engine::enc_ms::MsBands;
use crate::engine::enc_psy::AttackDetector;
use crate::engine::enc_quant::{MAX_BANDS, MAX_FLAT_SHORT, QuantChannel, QuantShort};
use crate::engine::enc_tns::EncTns;
use crate::engine::error::Result;
use crate::engine::ics::WindowSequence;

impl LcEncoder {
    /// Encode 1024 samples per channel into one `raw_data_block`. Causal
    /// path (lookahead off): the attack decision comes from THIS frame's
    /// samples.
    pub fn encode_frame(&mut self, pcm: &[&[f32]]) -> Result<Vec<u8>> {
        self.encode_into(pcm)?;
        Ok(self.payload.clone())
    }

    /// Encode into reused `payload` (TASK-78 streaming).
    pub fn encode_into(&mut self, pcm: &[&[f32]]) -> Result<()> {
        self.check_shape(pcm)?;
        let attack = self.detect(pcm);
        self.encode_with_attack(pcm, attack)
    }

    #[must_use]
    pub(crate) fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Zero overlap, rate credit, detectors, and lookahead. Prepared KBD
    /// windows, psy spreading matrices, and quant boxes stay allocated.
    pub(crate) fn reset(&mut self) {
        self.prev.iter_mut().for_each(|p| p.fill(0.0));
        let n_bands = self.chans[0].n_bands;
        self.chans[0] = QuantChannel::new(n_bands);
        self.chans[1] = QuantChannel::new(n_bands);
        self.books = [[0; MAX_BANDS]; 2];
        self.gains = [100; 2];
        self.target_q = [[0.0; MAX_BANDS]; 2];
        self.credit = 0;
        self.pad_debt = 0;
        if let Some(resv) = &mut self.cbr {
            resv.clear();
        }
        self.rate_guess = super::rate::OFFSET_LO;
        self.ms = MsBands::off();
        self.tns = [EncTns::off(), EncTns::off()];
        self.prev_seq = WindowSequence::OnlyLong;
        self.seq = WindowSequence::OnlyLong;
        self.detectors = [AttackDetector::new(), AttackDetector::new()];
        self.is_band = [false; MAX_BANDS];
        self.grouping = Grouping::ungrouped();
        self.held = None;
        self.payload.clear();
        self.fill.clear();
        self.pad_fill = 0;
        let n_sfb = self.chans_s[0].n_sfb;
        self.chans_s[0] = QuantShort::new(n_sfb);
        self.chans_s[1] = QuantShort::new(n_sfb);
        self.books_s = [[0; MAX_FLAT_SHORT]; 2];
        self.target_q_s = [[0.0; MAX_FLAT_SHORT]; 2];
    }
}
