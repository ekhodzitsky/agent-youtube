//! Small real AAC fragments with a complete direct-reference SIDX.
use std::ops::Range;

fn atom(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(body);
    out
}
fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|n| n.to_be_bytes()).collect()
}
fn find(data: &[u8], kind: &[u8; 4]) -> usize {
    data.windows(4).position(|w| w == kind).unwrap()
}
pub fn fragmented(rate: u32) -> Vec<u8> {
    let pcm: Vec<_> = (0..24 * rate)
        .map(|i| 0.2 * (i as f32 * 0.19).sin())
        .collect();
    let flat = syom::encode_with(
        &[&pcm],
        rate,
        &syom::EncodeOptions::default()
            .with_bitrate_bps(64000)
            .with_pns(true)
            .with_container(syom::EncodeContainer::M4a),
    )
    .unwrap();
    let stsz = find(&flat, b"stsz");
    let count = u32::from_be_bytes(flat[stsz + 12..stsz + 16].try_into().unwrap()) as usize;
    let sizes: Vec<_> = flat[stsz + 16..stsz + 16 + count * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| u32::from_be_bytes(*v))
        .collect();
    let moov_at = find(&flat, b"moov");
    let moov_len = u32::from_be_bytes(flat[moov_at - 4..moov_at].try_into().unwrap()) as usize;
    let mut moov = flat[moov_at + 4..moov_at - 4 + moov_len].to_vec();
    let edit = find(&moov, b"elst");
    moov[edit..edit + 4].copy_from_slice(b"free");
    moov.extend(atom(
        b"mvex",
        &atom(b"trex", &words(&[0, 1, 1, 1024, 0, 0])),
    ));
    let mut input = find(&flat, b"mdat") + 4;
    let mut fragments = Vec::new();
    let mut durations = Vec::new();
    for (i, group) in sizes.chunks((rate as usize * 8 / 1024).max(1)).enumerate() {
        let samples = group.len() as u32 * 1024;
        let mut run = words(&[0x301, group.len() as u32, 0]);
        for size in group {
            run.extend(words(&[1024, *size]));
        }
        let mut traf = atom(b"tfhd", &words(&[0x020000, 1]));
        traf.extend(atom(b"tfdt", &words(&[0, durations.iter().sum()])));
        traf.extend(atom(b"trun", &run));
        let mut body = atom(b"mfhd", &words(&[0, i as u32 + 1]));
        body.extend(atom(b"traf", &traf));
        let mut fragment = atom(b"moof", &body);
        let trun = find(&fragment, b"trun");
        let offset = fragment.len() as u32 + 8;
        fragment[trun + 12..trun + 16].copy_from_slice(&offset.to_be_bytes());
        let length: usize = group.iter().map(|n| *n as usize).sum();
        fragment.extend(atom(b"mdat", &flat[input..input + length]));
        input += length;
        durations.push(samples);
        fragments.push(fragment);
    }
    let mut index = words(&[0, 1, rate, 0, 0, fragments.len() as u32]);
    for (fragment, duration) in fragments.iter().zip(durations) {
        index.extend(words(&[fragment.len() as u32, duration, 0x9000_0000]));
    }
    let mut out = atom(b"ftyp", b"isom\0\0\0\0isom");
    out.extend(atom(b"moov", &moov));
    out.extend(atom(b"sidx", &index));
    for fragment in fragments {
        out.extend(fragment);
    }
    out
}
pub fn ranges(data: &[u8]) -> Vec<Range<usize>> {
    let mut starts = vec![0];
    let mut offset = 0;
    while offset < data.len() {
        let size = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        assert!(size >= 8 && offset + size <= data.len());
        if &data[offset + 4..offset + 8] == b"moof" {
            starts.push(offset);
        }
        offset += size;
    }
    starts.push(data.len());
    starts.windows(2).map(|s| s[0]..s[1]).collect()
}
