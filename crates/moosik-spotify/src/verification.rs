pub(crate) const REPUTATION_EXTENSION: u64 = 185;
const REPUTATION_TYPE: &[u8] =
    b"type.googleapis.com/spotify.contentagnostic.v2.OnPlatformReputationTrait";

enum Field<'a> {
    Number(u64),
    Bytes(&'a [u8]),
    Fixed,
}

fn varint(input: &mut &[u8]) -> Option<u64> {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let (&byte, rest) = input.split_first()?;
        *input = rest;
        if shift == 63 && byte > 1 {
            return None;
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Some(value);
        }
    }
    None
}

fn take<'a>(input: &mut &'a [u8], size: usize) -> Option<&'a [u8]> {
    let (data, rest) = input.split_at_checked(size)?;
    *input = rest;
    Some(data)
}

fn field(mut input: &[u8], wanted: u64) -> Option<Field<'_>> {
    let mut found = None;
    while !input.is_empty() {
        let tag = varint(&mut input)?;
        let number = tag >> 3;
        if number == 0 || number > 0x1fff_ffff {
            return None;
        }
        let value = match tag & 7 {
            0 => Field::Number(varint(&mut input)?),
            1 => {
                take(&mut input, 8)?;
                Field::Fixed
            }
            2 => {
                let size = usize::try_from(varint(&mut input)?).ok()?;
                Field::Bytes(take(&mut input, size)?)
            }
            5 => {
                take(&mut input, 4)?;
                Field::Fixed
            }
            _ => return None,
        };
        if number == wanted {
            found = Some(value);
        }
    }
    found
}

fn message(input: &[u8], number: u64) -> Option<&[u8]> {
    match field(input, number)? {
        Field::Bytes(data) => Some(data),
        _ => None,
    }
}

pub(crate) fn verified(response: &[u8], uri: &str) -> Option<bool> {
    let extension = message(response, 2)?;
    if !matches!(
        field(extension, 2),
        Some(Field::Number(REPUTATION_EXTENSION))
    ) {
        return None;
    }
    let entity = message(extension, 3)?;
    let header = message(entity, 1)?;
    if !matches!(field(header, 1), Some(Field::Number(200)))
        || message(entity, 2)? != uri.as_bytes()
    {
        return None;
    }
    let payload = message(entity, 3)?;
    if message(payload, 1)? != REPUTATION_TYPE {
        return None;
    }
    let reputation = message(payload, 2)?;
    let verification = message(reputation, 9)?;
    match field(verification, 2)? {
        Field::Number(0) => Some(false),
        Field::Number(1) => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(number: u8, data: &[u8]) -> Vec<u8> {
        assert!(data.len() < 128);
        let mut field = vec![number << 3 | 2, data.len() as u8];
        field.extend_from_slice(data);
        field
    }

    fn response(status: &[u8], verification: Option<u8>) -> Vec<u8> {
        let mut reputation = vec![16, 1];
        if let Some(verification) = verification {
            reputation.extend(bytes(9, &[8, 1, 16, verification]));
        }
        let mut payload = bytes(1, REPUTATION_TYPE);
        payload.extend(bytes(2, &reputation));
        let mut entity = bytes(1, status);
        entity.extend(bytes(2, b"spotify:artist:a"));
        entity.extend(bytes(3, &payload));
        let mut extension = vec![16, 185, 1];
        extension.extend(bytes(3, &entity));
        bytes(2, &extension)
    }

    #[test]
    fn verification_reads_the_current_flag_only() {
        let uri = "spotify:artist:a";
        let status = [8, 200, 1];
        assert_eq!(verified(&response(&status, Some(1)), uri), Some(true));
        assert_eq!(verified(&response(&status, Some(0)), uri), Some(false));
        assert_eq!(verified(&response(&status, None), uri), None);
    }

    #[test]
    fn verification_requires_matching_successful_metadata() {
        let mut data = response(&[8, 200, 1], Some(1));
        assert_eq!(verified(&data, "spotify:artist:b"), None);
        assert_eq!(
            verified(&response(&[8, 148, 3], Some(1)), "spotify:artist:a"),
            None
        );
        data.pop();
        assert_eq!(verified(&data, "spotify:artist:a"), None);
    }
}
