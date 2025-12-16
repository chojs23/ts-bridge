use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Low,
    Normal,
    Const,
}

#[derive(Debug)]
pub struct Request {
    pub seq: u64,
    pub payload: serde_json::Value,
    pub priority: Priority,
}

#[derive(Debug, Default)]
pub struct RequestQueue {
    seq: u64,
    queue: VecDeque<Request>,
}

impl RequestQueue {
    pub fn enqueue(&mut self, mut payload: serde_json::Value, priority: Priority) -> u64 {
        let seq = self.next_seq();
        assign_seq(&mut payload, seq);
        let request = Request {
            seq,
            payload,
            priority,
        };

        match priority {
            Priority::Const => self.queue.push_front(request),
            Priority::Low => self.queue.push_back(request),
            Priority::Normal => {
                let idx = self
                    .queue
                    .iter()
                    .rposition(|req| matches!(req.priority, Priority::Const))
                    .map(|pos| pos + 1)
                    .unwrap_or(0);
                self.queue.insert(idx, request);
            }
        }

        seq
    }

    pub fn dequeue(&mut self) -> Option<Request> {
        self.queue.pop_front()
    }

    fn next_seq(&mut self) -> u64 {
        let seq = self.seq;
        self.seq += 1;
        seq
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

fn assign_seq(payload: &mut serde_json::Value, seq: u64) {
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("seq".to_string(), serde_json::json!(seq));
    }
}
