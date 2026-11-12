use std::collections::HashSet;

pub const MAGIC: &[u8; 4] = b"WTCH";
pub const ACK_MAGIC: &[u8; 4] = b"WTOK";
pub const VERSION: u8 = 2;
pub const HEADER_LEN: usize = 12;
pub const POINTER_LEN: usize = 20;
pub const MAX_POINTERS: usize = 10;
pub const MAX_POINTER_ID: u8 = 31;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Action {
    Down = 0,
    Up = 1,
    Move = 2,
    Cancel = 3,
    PointerDown = 5,
    PointerUp = 6,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pointer {
    pub id: u8,
    pub tool_type: u8,
    pub x: f32,
    pub y: f32,
    pub pressure: f32,
    pub size: f32,
}

impl Pointer {
    pub fn finger(id: u8, x: f32, y: f32) -> Self {
        Self {
            id,
            tool_type: 1,
            x,
            y,
            pressure: 1.0,
            size: 1.0,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.id > MAX_POINTER_ID {
            return Err(format!(
                "pointer id {} exceeds Android range 0..31",
                self.id
            ));
        }
        if !matches!(self.tool_type, 1 | 2 | 4) {
            return Err(format!(
                "unsupported touchscreen tool type {}",
                self.tool_type
            ));
        }
        if !self.x.is_finite() || !self.y.is_finite() || self.x < 0.0 || self.y < 0.0 {
            return Err("coordinates must be finite and non-negative".into());
        }
        if !self.pressure.is_finite() || !(0.0..=1.0).contains(&self.pressure) {
            return Err("pressure must be in [0, 1]".into());
        }
        if !self.size.is_finite() || !(0.0..=1.0).contains(&self.size) {
            return Err("size must be in [0, 1]".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub sequence: u32,
    pub action: Action,
    pub action_index: u8,
    pub pointers: Vec<Pointer>,
}

impl Frame {
    pub fn validate_basic(&self) -> Result<(), String> {
        let count = self.pointers.len();
        if count == 0 || count > MAX_POINTERS {
            return Err(format!(
                "pointer count must be 1..={MAX_POINTERS}, got {count}"
            ));
        }
        let mut ids = HashSet::with_capacity(count);
        for pointer in &self.pointers {
            pointer.validate()?;
            if !ids.insert(pointer.id) {
                return Err(format!("duplicate pointer id {}", pointer.id));
            }
        }
        match self.action {
            Action::Down | Action::Up | Action::Move | Action::Cancel if self.action_index != 0 => {
                return Err(format!("{:?} requires action_index=0", self.action));
            }
            Action::PointerDown | Action::PointerUp if usize::from(self.action_index) >= count => {
                return Err("action_index is outside pointer_count".into());
            }
            _ => {}
        }
        match self.action {
            Action::Down | Action::Up if count != 1 => {
                return Err("DOWN/UP requires exactly one pointer".into());
            }
            Action::PointerDown | Action::PointerUp if count < 2 => {
                return Err("POINTER_DOWN/UP requires at least two pointers".into());
            }
            _ => {}
        }
        Ok(())
    }

    /// Validate the transition and return the active pointer set after this event.
    pub fn next_active(&self, active: &[Pointer]) -> Result<Vec<Pointer>, String> {
        self.validate_basic()?;
        let ids = |items: &[Pointer]| items.iter().map(|p| p.id).collect::<Vec<_>>();
        let current_ids = ids(&self.pointers);
        let active_ids = ids(active);
        match self.action {
            Action::Down => {
                if !active.is_empty() {
                    return Err("DOWN while pointers are active".into());
                }
                Ok(self.pointers.clone())
            }
            Action::PointerDown => {
                if active.is_empty() || self.pointers.len() != active.len() + 1 {
                    return Err("POINTER_DOWN must add exactly one pointer".into());
                }
                let index = usize::from(self.action_index);
                let mut old_order = current_ids.clone();
                old_order.remove(index);
                if old_order != active_ids {
                    return Err("POINTER_DOWN must preserve existing pointer order".into());
                }
                if active_ids.contains(&current_ids[index]) {
                    return Err("POINTER_DOWN used an already-active pointer ID".into());
                }
                Ok(self.pointers.clone())
            }
            Action::Move => {
                if active.is_empty() || current_ids != active_ids {
                    return Err("MOVE must contain exactly the active IDs in the same order".into());
                }
                Ok(self.pointers.clone())
            }
            Action::PointerUp => {
                if active.len() < 2 || current_ids != active_ids {
                    return Err("POINTER_UP must contain the complete active set in order".into());
                }
                let mut next = self.pointers.clone();
                next.remove(usize::from(self.action_index));
                Ok(next)
            }
            Action::Up => {
                if active.len() != 1 || current_ids != active_ids {
                    return Err("UP requires the sole active pointer".into());
                }
                Ok(Vec::new())
            }
            Action::Cancel => {
                if active.is_empty() || current_ids != active_ids {
                    return Err("CANCEL must contain the complete active set".into());
                }
                Ok(Vec::new())
            }
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate_basic()?;
        let mut out = Vec::with_capacity(HEADER_LEN + self.pointers.len() * POINTER_LEN);
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.push(self.action as u8);
        out.push(self.pointers.len() as u8);
        out.push(self.action_index);
        out.extend_from_slice(&self.sequence.to_le_bytes());
        for pointer in &self.pointers {
            out.push(pointer.id);
            out.push(pointer.tool_type);
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&pointer.x.to_le_bytes());
            out.extend_from_slice(&pointer.y.to_le_bytes());
            out.extend_from_slice(&pointer.pressure.to_le_bytes());
            out.extend_from_slice(&pointer.size.to_le_bytes());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: u8, x: f32) -> Pointer {
        Pointer::finger(id, x, x + 1.0)
    }

    #[test]
    fn v2_frame_has_exact_wire_layout() {
        let frame = Frame {
            sequence: 0x1234_5678,
            action: Action::Down,
            action_index: 0,
            pointers: vec![p(3, 10.0)],
        };
        let bytes = frame.encode().unwrap();
        assert_eq!(bytes.len(), HEADER_LEN + POINTER_LEN);
        assert_eq!(&bytes[0..4], b"WTCH");
        assert_eq!(bytes[4], 2);
        assert_eq!(bytes[5], 0);
        assert_eq!(bytes[6], 1);
        assert_eq!(
            u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
            0x1234_5678
        );
        assert_eq!(bytes[12], 3);
        assert_eq!(bytes[13], 1);
        assert_eq!(f32::from_le_bytes(bytes[16..20].try_into().unwrap()), 10.0);
    }

    #[test]
    fn three_pointer_gesture_transitions_are_valid() {
        let mut active = Vec::new();
        let down = Frame {
            sequence: 1,
            action: Action::Down,
            action_index: 0,
            pointers: vec![p(0, 10.0)],
        };
        active = down.next_active(&active).unwrap();
        let add5 = Frame {
            sequence: 2,
            action: Action::PointerDown,
            action_index: 1,
            pointers: vec![p(0, 11.0), p(5, 20.0)],
        };
        active = add5.next_active(&active).unwrap();
        let add9 = Frame {
            sequence: 3,
            action: Action::PointerDown,
            action_index: 2,
            pointers: vec![p(0, 12.0), p(5, 21.0), p(9, 30.0)],
        };
        active = add9.next_active(&active).unwrap();
        assert_eq!(
            active.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![0, 5, 9]
        );
        let mv = Frame {
            sequence: 4,
            action: Action::Move,
            action_index: 0,
            pointers: vec![p(0, 13.0), p(5, 22.0), p(9, 31.0)],
        };
        active = mv.next_active(&active).unwrap();
        let up5 = Frame {
            sequence: 5,
            action: Action::PointerUp,
            action_index: 1,
            pointers: active.clone(),
        };
        active = up5.next_active(&active).unwrap();
        assert_eq!(active.iter().map(|p| p.id).collect::<Vec<_>>(), vec![0, 9]);
        let up9 = Frame {
            sequence: 6,
            action: Action::PointerUp,
            action_index: 1,
            pointers: active.clone(),
        };
        active = up9.next_active(&active).unwrap();
        let up0 = Frame {
            sequence: 7,
            action: Action::Up,
            action_index: 0,
            pointers: active.clone(),
        };
        active = up0.next_active(&active).unwrap();
        assert!(active.is_empty());
    }

    #[test]
    fn rejects_duplicate_ids_reordered_moves_and_bad_transitions() {
        let duplicate = Frame {
            sequence: 1,
            action: Action::PointerDown,
            action_index: 1,
            pointers: vec![p(2, 1.0), p(2, 2.0)],
        };
        assert!(duplicate.validate_basic().is_err());
        let active = vec![p(0, 1.0), p(1, 2.0)];
        let reordered = Frame {
            sequence: 2,
            action: Action::Move,
            action_index: 0,
            pointers: vec![p(1, 2.0), p(0, 1.0)],
        };
        assert!(reordered.next_active(&active).is_err());
        let move_without_down = Frame {
            sequence: 3,
            action: Action::Move,
            action_index: 0,
            pointers: vec![p(0, 1.0)],
        };
        assert!(move_without_down.next_active(&[]).is_err());
    }

    #[test]
    fn rejects_pointer_overflow_and_non_finite_values() {
        let too_many = (0..11).map(|id| p(id, id as f32)).collect();
        let frame = Frame {
            sequence: 1,
            action: Action::Move,
            action_index: 0,
            pointers: too_many,
        };
        assert!(frame.validate_basic().is_err());
        let nan = Frame {
            sequence: 1,
            action: Action::Down,
            action_index: 0,
            pointers: vec![Pointer::finger(0, f32::NAN, 1.0)],
        };
        assert!(nan.validate_basic().is_err());
    }

    #[test]
    fn accepts_ten_pointers_and_cancels_the_complete_active_set() {
        let mut active = vec![p(0, 1.0)];
        let down = Frame {
            sequence: 1,
            action: Action::Down,
            action_index: 0,
            pointers: active.clone(),
        };
        active = down.next_active(&[]).unwrap();

        for id in 1..10u8 {
            let mut pointers = active.clone();
            pointers.push(p(id, f32::from(id)));
            let frame = Frame {
                sequence: u32::from(id) + 1,
                action: Action::PointerDown,
                action_index: id,
                pointers,
            };
            active = frame.next_active(&active).unwrap();
        }
        assert_eq!(active.len(), MAX_POINTERS);

        let cancel = Frame {
            sequence: 11,
            action: Action::Cancel,
            action_index: 0,
            pointers: active.clone(),
        };
        assert!(cancel.next_active(&active).unwrap().is_empty());
    }

    #[test]
    fn pointer_down_can_insert_at_a_non_terminal_index() {
        let active = vec![p(0, 1.0), p(9, 9.0)];
        let frame = Frame {
            sequence: 3,
            action: Action::PointerDown,
            action_index: 1,
            pointers: vec![p(0, 1.0), p(5, 5.0), p(9, 9.0)],
        };
        let next = frame.next_active(&active).unwrap();
        assert_eq!(
            next.iter().map(|pointer| pointer.id).collect::<Vec<_>>(),
            vec![0, 5, 9]
        );
    }
}
