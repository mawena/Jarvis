//! Trames binaires du WebSocket : `[type: u8][phrase: u32 LE][données…]`.
//! Seul type en v1 : le son du micro (`0x01`), i16 LE mono 16 kHz.

/// Type de trame : son du micro.
pub const MIC_AUDIO: u8 = 0x01;
/// Échantillons au plus par trame (1 s).
pub const MAX_FRAME_SAMPLES: usize = 16_000;
/// Son d'une phrase envoyé au plus par l'oreille, de la première trame à la fin de
/// phrase (seconde de tête comprise) : 14 s à 16 kHz. L'oreille borne ses captures
/// (réveil et réponse à une question) à cette longueur ; le cerveau garde un peu plus.
pub const MAX_PHRASE_SAMPLES: usize = 14 * 16_000;
/// Type et numéro de phrase.
const HEADER_LEN: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    MicAudio {
        utterance: u32,
        samples: Vec<i16>,
    },
    /// Type inconnu (version plus récente) : à ignorer.
    Unknown(u8),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("trame vide")]
    Empty,
    #[error("trame tronquée")]
    Truncated,
    #[error("son de longueur impaire")]
    OddLength,
    #[error("trame trop grande")]
    TooLarge,
}

/// Trame de son du micro pour la phrase `utterance`.
pub fn encode_mic_frame(utterance: u32, samples: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + 2 * samples.len());
    out.push(MIC_AUDIO);
    out.extend_from_slice(&utterance.to_le_bytes());
    out.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
    out
}

/// Lit une trame. Un type inconnu n'est pas une erreur : `Frame::Unknown`.
pub fn decode_frame(bytes: &[u8]) -> Result<Frame, FrameError> {
    let (&kind, rest) = bytes.split_first().ok_or(FrameError::Empty)?;
    if kind != MIC_AUDIO {
        return Ok(Frame::Unknown(kind));
    }
    let (utterance, pcm) = rest.split_first_chunk::<4>().ok_or(FrameError::Truncated)?;
    if pcm.len() % 2 != 0 {
        return Err(FrameError::OddLength);
    }
    if pcm.len() / 2 > MAX_FRAME_SAMPLES {
        return Err(FrameError::TooLarge);
    }
    Ok(Frame::MicAudio {
        utterance: u32::from_le_bytes(*utterance),
        samples: pcm
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trame_audio_aller_retour() {
        let samples = [0, 1, -1, i16::MAX, i16::MIN];
        let bytes = encode_mic_frame(0x0102_0304, &samples);
        assert_eq!(bytes[..5], [0x01, 0x04, 0x03, 0x02, 0x01]);
        assert_eq!(bytes[5..7], [0x00, 0x00]);
        assert_eq!(bytes.len(), 5 + 2 * samples.len());
        assert_eq!(
            decode_frame(&bytes),
            Ok(Frame::MicAudio {
                utterance: 0x0102_0304,
                samples: samples.to_vec(),
            })
        );
        // Trame sans son : permise.
        assert_eq!(
            decode_frame(&encode_mic_frame(7, &[])),
            Ok(Frame::MicAudio {
                utterance: 7,
                samples: vec![],
            })
        );
        let full = vec![1i16; MAX_FRAME_SAMPLES];
        assert!(decode_frame(&encode_mic_frame(1, &full)).is_ok());
    }

    #[test]
    fn trame_audio_impaire_refusee() {
        let mut bytes = encode_mic_frame(1, &[1, 2]);
        bytes.pop();
        assert_eq!(decode_frame(&bytes), Err(FrameError::OddLength));
        assert_eq!(decode_frame(&[]), Err(FrameError::Empty));
        assert_eq!(decode_frame(&[MIC_AUDIO, 1, 0]), Err(FrameError::Truncated));
    }

    #[test]
    fn trame_trop_grande_refusee() {
        let samples = vec![0i16; MAX_FRAME_SAMPLES + 1];
        assert_eq!(
            decode_frame(&encode_mic_frame(1, &samples)),
            Err(FrameError::TooLarge)
        );
    }

    #[test]
    fn trame_type_inconnu() {
        assert_eq!(decode_frame(&[0x7f]), Ok(Frame::Unknown(0x7f)));
        assert_eq!(decode_frame(&[0x02, 1, 2, 3]), Ok(Frame::Unknown(0x02)));
    }
}
