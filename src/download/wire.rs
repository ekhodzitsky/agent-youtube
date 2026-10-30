//! Bounded readers for the SABR protobuf messages and UMP framing.
use crate::Result;
use std::io::Read;

pub fn varint(data: &mut &[u8]) -> Result<u64> {
    let mut value = 0_u64;
    for shift in (0..70).step_by(7) {
        let byte = *data.first().ok_or("truncated protobuf integer")?;
        *data = &data[1..];
        if shift == 63 && byte > 1 {
            return Err("protobuf integer overflow".into());
        }
        value |= u64::from(byte & 127) << shift;
        if byte & 128 == 0 {
            return Ok(value);
        }
    }
    Err("protobuf integer overflow".into())
}
#[derive(Clone, Copy)]
pub enum Value<'a> {
    Number(u64),
    Bytes(&'a [u8]),
    Fixed,
}
pub fn fields(mut data: &[u8]) -> Result<Vec<(u32, Value<'_>)>> {
    let mut fields = Vec::new();
    while !data.is_empty() {
        if fields.len() >= 4096 {
            return Err("too many protobuf fields".into());
        }
        let tag = varint(&mut data)?;
        let key = u32::try_from(tag >> 3)?;
        if key == 0 {
            return Err("zero protobuf field number".into());
        }
        let value = match tag & 7 {
            0 => Value::Number(varint(&mut data)?),
            1 | 5 => {
                let len = if tag & 7 == 1 { 8 } else { 4 };
                data = data.get(len..).ok_or("truncated fixed protobuf field")?;
                Value::Fixed
            }
            2 => {
                let len = usize::try_from(varint(&mut data)?)?;
                let bytes = data.get(..len).ok_or("truncated protobuf field")?;
                data = &data[len..];
                Value::Bytes(bytes)
            }
            _ => return Err("unsupported protobuf wire type".into()),
        };
        fields.push((key, value));
    }
    Ok(fields)
}
pub fn number(data: &[(u32, Value<'_>)], key: u32) -> Option<u64> {
    data.iter().rev().find_map(|(k, v)| match v {
        Value::Number(n) if *k == key => Some(*n),
        _ => None,
    })
}
pub fn bytes<'a>(data: &[(u32, Value<'a>)], key: u32) -> Option<&'a [u8]> {
    data.iter().rev().find_map(|(k, v)| match v {
        Value::Bytes(b) if *k == key => Some(*b),
        _ => None,
    })
}
pub fn put_var(out: &mut Vec<u8>, mut n: u64) {
    while n > 127 {
        out.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    out.push(n as u8);
}
pub fn num(out: &mut Vec<u8>, key: u32, n: u64) {
    put_var(out, u64::from(key) * 8);
    put_var(out, n);
}
pub fn blob(out: &mut Vec<u8>, key: u32, value: &[u8]) {
    put_var(out, u64::from(key) * 8 + 2);
    put_var(out, value.len() as u64);
    out.extend_from_slice(value);
}
fn ump_number<R: Read>(reader: &mut R, first: u8) -> Result<u32> {
    let count = if first < 128 {
        0
    } else if first < 192 {
        1
    } else if first < 224 {
        2
    } else if first < 240 {
        3
    } else {
        4
    };
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes[..count])?;
    let high = u32::from_le_bytes(bytes);
    Ok(if count == 4 {
        high
    } else {
        u32::from(first & (127 >> count)) + (high << (7 - count))
    })
}
pub fn part<R: Read>(reader: &mut R) -> Result<Option<(u32, Vec<u8>)>> {
    let mut first = [0];
    if reader.read(&mut first)? == 0 {
        return Ok(None);
    }
    let kind = ump_number(reader, first[0])?;
    reader.read_exact(&mut first)?;
    let len = ump_number(reader, first[0])? as usize;
    if len > 8 * 1024 * 1024 {
        return Err("UMP part exceeds byte limit".into());
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    Ok(Some((kind, body)))
}
