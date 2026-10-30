#![allow(clippy::unwrap_used)]

#[test]
fn fractional_finish_requires_grid_padding() {
    let from = bandlimit::Rate::new(22050).unwrap();
    let to = bandlimit::Rate::new(16000).unwrap();
    let mut stream = bandlimit::Stream::new(from, to).unwrap();
    let mut input: Vec<_> = (0..22063).map(|i| 0.2 * (i as f32 * 0.19).sin()).collect();
    let mut output = Vec::with_capacity(80000);
    for block in input.chunks(511) {
        stream.write(block, &mut output).unwrap();
    }
    // output_len explicitly rejects non-integral rational lengths. This is
    // a caller precondition, not a finalization defect or numeric overflow.
    assert_eq!(stream.finish(), Err(bandlimit::Error::Overflow));
    let quantum = bandlimit::Ratio::reduce(from, to).numerator() as usize;
    let padded = input.len().div_ceil(quantum) * quantum;
    stream
        .write(&vec![0.0; padded - input.len()], &mut output)
        .unwrap();
    output.extend(stream.finish().unwrap());
    assert!(stream.finish().unwrap().is_empty());
    input.resize(padded, 0.0);
    let expected = bandlimit::convert(&input, from, to).unwrap();
    assert_eq!(output.len(), padded * 16000 / 22050);
    assert_eq!(
        output.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
        expected.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
    );
}
