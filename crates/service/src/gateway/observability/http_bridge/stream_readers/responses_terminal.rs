use super::OpenAIResponsesEvent;

/// Track complete, valid Responses SSE terminal frames in raw forwarded bytes.
/// Never treat an event name or a string inside an output delta as completion.
#[derive(Default)]
pub(super) struct ForwardedResponsesTerminal {
    line: Vec<u8>,
    frame: Vec<String>,
    seen: bool,
}

impl ForwardedResponsesTerminal {
    pub(super) fn seen(&self) -> bool {
        self.seen
    }

    pub(super) fn feed(&mut self, bytes: &[u8]) {
        if self.seen {
            return;
        }
        for &byte in bytes {
            self.line.push(byte);
            if byte != b'\n' {
                continue;
            }
            let line = std::mem::take(&mut self.line);
            let blank = line == b"\n" || line == b"\r\n";
            match String::from_utf8(line) {
                Ok(line) => self.frame.push(line),
                Err(_) => {
                    self.frame.clear();
                    continue;
                }
            }
            if blank {
                let event = OpenAIResponsesEvent::parse(&self.frame);
                self.frame.clear();
                if event.is_some_and(|event| event.terminal.is_some()) {
                    self.seen = true;
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_name_inside_delta_or_partial_frame_is_not_terminal() {
        let mut tracker = ForwardedResponsesTerminal::default();
        tracker.feed(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"response.completed\"}\n\n",
        );
        assert!(!tracker.seen());
        tracker.feed(b"event: response.completed\ndata: {\"type\":\"response.completed\"}");
        assert!(!tracker.seen());
        tracker.feed(b"\n");
        assert!(!tracker.seen());
        tracker.feed(b"\n");
        assert!(tracker.seen());
    }

    #[test]
    fn every_chunk_boundary_preserves_completed_and_error_frame_detection() {
        for frame in [
            &b"event: response.completed\r\ndata: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\r\n\r\n"[..],
            &b"data: {\"type\":\"error\",\"error\":{\"type\":\"internal_error\",\"message\":\"response protection is unavailable\"}}\n\n"[..],
            &b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\",\"message\":\"test failure\"}}}\n\n"[..],
        ] {
            for split in 0..frame.len() {
                let mut tracker = ForwardedResponsesTerminal::default();
                tracker.feed(&frame[..split]);
                assert!(!tracker.seen());
                tracker.feed(&frame[split..]);
                assert!(tracker.seen());
            }
        }
    }

    #[test]
    fn invalid_json_and_non_terminal_tool_completion_do_not_end_response() {
        let mut tracker = ForwardedResponsesTerminal::default();
        tracker.feed(b"event: response.completed\ndata: invalid\n\n");
        tracker.feed(b"data: {\"type\":\"response.web_search_call.completed\"}\n\n");
        assert!(!tracker.seen());
    }
}
