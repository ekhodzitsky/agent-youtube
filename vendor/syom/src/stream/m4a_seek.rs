//! Seekable M4A: load `moov`, then `seek`+read each sample. `mdat` is not
//! slurped. Presentation-sample seeking is [`super::m4a_pos::M4aSeek`].
//!
//! ```
//! use std::io::Cursor;
//! use syom::{DecodeOptions, decode_seek_streaming};
//! let m4a = include_bytes!("../goldens/sine441.m4a");
//! let mut samples = 0usize;
//! let info = decode_seek_streaming(Cursor::new(m4a), &DecodeOptions::speech(), |f| {
//!     samples += f.samples;
//!     Ok(())
//! })?;
//! assert_eq!(info.sample_rate, 44_100);
//! assert_eq!(samples as u64, info.samples);
//! # Ok::<(), syom::AacError>(())
//! ```

use std::io::{self, Read, Seek, SeekFrom};

use crate::error::{AacError, Result};
use crate::isomp4::{self, TrackBoxes, load_track_boxes};
use crate::options::DecodeOptions;
use crate::stream::{Frame, StreamInfo};

use super::m4a::{map_m4a_parse_err, play_m4a_track};

/// Stream-decode M4A from a seekable reader. Peak compressed RAM is the
/// `moov` box plus one access unit (`mdat` is skipped, then sampled).
pub fn decode_seek_streaming<R, F>(
    mut reader: R,
    opts: &DecodeOptions,
    on_frame: F,
) -> Result<StreamInfo>
where
    R: Read + Seek,
    F: FnMut(Frame<'_>) -> Result<()>,
{
    opts.validate()?;
    stream_m4a_seek(&mut reader, opts, on_frame)
}

fn read_exact<R: Read>(r: &mut R, buf: &mut [u8]) -> Result<()> {
    loop {
        match r.read_exact(buf) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(AacError::truncated_at(None));
            }
            Err(e) => return Err(AacError::from(e)),
        }
    }
}

/// Decode M4A from a seekable reader. Peak compressed RAM is the `moov` box
/// plus one access unit.
pub(crate) fn stream_m4a_seek<R, F>(
    reader: &mut R,
    opts: &DecodeOptions,
    on_frame: F,
) -> Result<StreamInfo>
where
    R: Read + Seek,
    F: FnMut(Frame<'_>) -> Result<()>,
{
    let boxes = load_track_boxes(reader, &opts.memory)?;
    let gap = match &boxes {
        TrackBoxes::Flat { moov, .. } | TrackBoxes::Frag { moov, .. } => {
            crate::gapless::itunsmpb_in(moov)
        }
    };
    let track = match boxes {
        TrackBoxes::Flat { moov, file_len } => {
            isomp4::parse_aac_track_with_len(&moov, file_len, &opts.memory)
                .map_err(map_m4a_parse_err)?
        }
        TrackBoxes::Frag {
            moov, moofs, mdats, ..
        } => isomp4::frag::track_from_parts(&moov, &moofs, &mdats, &opts.memory)?,
    };
    play_m4a_track(
        track,
        gap,
        opts,
        |off, len| {
            let n =
                usize::try_from(len).map_err(|_| AacError::format("aac: sample range overflow"))?;
            reader.seek(SeekFrom::Start(off)).map_err(AacError::from)?;
            let mut buf = vec![0u8; n];
            read_exact(reader, &mut buf)?;
            Ok(buf)
        },
        on_frame,
    )
}
