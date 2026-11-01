/// Strip terminal control sequences and C0 controls from guest output before
/// it reaches the host console. Build logs keep newlines, tabs, carriage
/// returns and UTF-8; ANSI/OSC/DCS sequences are discarded.
#[derive(Default)]
pub(crate) struct Sanitizer {
    state: SanitizerState,
}

#[derive(Default)]
enum SanitizerState {
    #[default]
    Normal,
    Escape,
    Csi,
    EscapeIntermediate,
    Osc,
    String,
    OscEscape,
}

impl Sanitizer {
    pub(crate) fn push(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        for &byte in bytes {
            match self.state {
                SanitizerState::Normal => match byte {
                    0x1b => self.state = SanitizerState::Escape,
                    b'\n' | b'\r' | b'\t' => out.push(byte),
                    0x00..=0x1f | 0x7f => {}
                    _ => out.push(byte),
                },
                SanitizerState::Escape => match byte {
                    b'[' => self.state = SanitizerState::Csi,
                    b']' => self.state = SanitizerState::Osc,
                    b'P' | b'X' | b'^' | b'_' => self.state = SanitizerState::String,
                    0x20..=0x2f => self.state = SanitizerState::EscapeIntermediate,
                    _ => self.state = SanitizerState::Normal,
                },
                SanitizerState::EscapeIntermediate => {
                    if (0x30..=0x7e).contains(&byte) {
                        self.state = SanitizerState::Normal;
                    }
                }
                SanitizerState::Csi => {
                    if (0x40..=0x7e).contains(&byte) {
                        self.state = SanitizerState::Normal;
                    }
                }
                SanitizerState::Osc | SanitizerState::String => match byte {
                    0x07 => self.state = SanitizerState::Normal,
                    0x1b => self.state = SanitizerState::OscEscape,
                    _ => {}
                },
                SanitizerState::OscEscape => match byte {
                    b'\\' => self.state = SanitizerState::Normal,
                    _ => self.state = SanitizerState::Osc,
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Sanitizer;

    #[test]
    fn sanitizer_strips_terminal_controls() {
        let mut sanitizer = Sanitizer::default();
        let mut out = Vec::new();
        sanitizer.push(
            b"\x1b]52;c;Zm9v\x07hello\x1b[31mred\x1b[0m\x08\x1bPabc\x1b\\!",
            &mut out,
        );
        assert_eq!(out, b"hellored!");
    }

    #[test]
    fn sanitizer_consumes_intermediate_escape_bytes_across_chunks() {
        let mut sanitizer = Sanitizer::default();
        let mut out = Vec::new();
        sanitizer.push(b"before\x1b(", &mut out);
        sanitizer.push(b"Bafter", &mut out);
        assert_eq!(out, b"beforeafter");
    }

    #[test]
    fn sanitizer_preserves_utf8_and_newlines() {
        let mut sanitizer = Sanitizer::default();
        let mut out = Vec::new();
        sanitizer.push(b"h\xc3\xa9llo\n", &mut out);
        assert_eq!(out, b"h\xc3\xa9llo\n");
    }
}
