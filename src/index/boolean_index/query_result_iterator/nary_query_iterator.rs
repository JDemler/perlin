use std::cell::RefCell;

use utils::owning_iterator::{SeekingIterator, OwningIterator};

use index::boolean_index::boolean_query::*;
use index::boolean_index::posting::Posting;
use index::boolean_index::query_result_iterator::*;

pub struct NAryQueryIterator {
    pos_operator: Option<PositionalOperator>,
    bool_operator: Option<BooleanOperator>,
    operands: Vec<QueryResultIterator>,
    peeked_value: RefCell<Option<Option<*const Posting>>>,
}


impl<'a> OwningIterator<'a> for NAryQueryIterator {
    type Item = &'a Posting;


    fn next(&'a self) -> Option<Self::Item> {
        let mut peeked_value = self.peeked_value.borrow_mut();
        if peeked_value.is_some() {
            return unsafe { peeked_value.take().unwrap().map(|p| &*p) };
        }
        match self.bool_operator {
            Some(BooleanOperator::And) => self.next_and(),
            Some(BooleanOperator::Or) => self.next_or(),
            None => {
                match self.pos_operator {
                    Some(PositionalOperator::InOrder) => self.next_inorder(),
                    None => {
                        unreachable!(false);
                    }
                }
            }
        }
    }

    fn peek(&'a self) -> Option<&'a Posting> {
        let mut peeked_value = self.peeked_value.borrow_mut();
        if peeked_value.is_none() {
            *peeked_value = Some((match self.bool_operator {
                    Some(BooleanOperator::And) => self.next_and(),
                    Some(BooleanOperator::Or) => self.next_or(),
                    None => {
                        match self.pos_operator {
                            Some(PositionalOperator::InOrder) => self.next_inorder(),
                            None => {
                                unreachable!(false);
                            }
                        }
                    }
                })
                .map(|p| p as *const Posting))
        }
        unsafe { peeked_value.unwrap().map(|p| &*p) }
    }

    fn len(&self) -> usize {
        self.estimate_length()
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<'a> SeekingIterator<'a> for NAryQueryIterator {
    type Item = &'a Posting;

    fn next_seek(&'a self, target: &Posting) -> Option<Self::Item> {
        {
            let mut peeked_value = self.peeked_value.borrow_mut();
            if peeked_value.is_some() {
                *peeked_value = None;
            }
            // Advance operands
            for op in self.operands.iter() {
                op.peek_seek(target);
            }
        }
        self.next()
    }

    fn peek_seek(&'a self, target: &Posting) -> Option<Self::Item> {
        let mut peeked_value = self.peeked_value.borrow_mut();
        if peeked_value.is_none() {
            *peeked_value = Some(self.next_seek(target).map(|p| p as *const Posting));
        }
        unsafe { peeked_value.unwrap().map(|p| &*p) }
    }
}

impl NAryQueryIterator {
    pub fn new_positional(operator: PositionalOperator, operands: Vec<QueryResultIterator>) -> NAryQueryIterator {
        let mut result = NAryQueryIterator {
            pos_operator: Some(operator),
            bool_operator: None,
            operands: operands,
            peeked_value: RefCell::new(None),
        };
        result.operands.sort_by_key(|op| op.estimate_length());
        result
    }


    pub fn new(operator: BooleanOperator, operands: Vec<QueryResultIterator>) -> NAryQueryIterator {
        let mut result = NAryQueryIterator {
            pos_operator: None,
            bool_operator: Some(operator),
            operands: operands,
            peeked_value: RefCell::new(None),
        };
        result.operands.sort_by_key(|op| op.estimate_length());
        result
    }

    pub fn estimate_length(&self) -> usize {
        match self.bool_operator {
            Some(BooleanOperator::And) => self.operands[0].estimate_length(),
            Some(BooleanOperator::Or) => self.operands[self.operands.len() - 1].estimate_length(),
            None => {
                match self.pos_operator {
                    Some(PositionalOperator::InOrder) => self.operands[0].estimate_length(),
                    None => {
                        unreachable!();
                    }
                }
            }
        }
    }

    fn next_inorder(&self) -> Option<&Posting> {
        let mut focus = None; //Acts as temporary to be compared against
        let mut focus_positions = vec![];
        // The iterator index that last set 'focus'
        let mut last_doc_iter = self.operands.len() + 1;
        // The the relative position of last_doc_iter
        let mut last_positions_iter = self.operands.len() + 1;
        'possible_documents: loop {
            // For every term
            for (i, input) in self.operands.iter().enumerate() {
                'term_documents: loop {
                    // If the focus was set by the current iterator, we have a match
                    if last_doc_iter == i {
                        break 'term_documents;
                    }
                    // Get the next doc_id for the current iterator
                    let mut v = try_option!(input.next());
                    if focus.is_none() {
                        focus = Some(v);
                        focus_positions = v.1.clone();
                        last_doc_iter = i;
                        last_positions_iter = input.relative_position();
                        break 'term_documents;
                    } else if v.0 < focus.unwrap().0 {
                        // If the doc_id is smaller, get its next value
                        continue 'term_documents;
                    } else if v.0 == focus.unwrap().0 {
                        // If the doc_id is equal, check positions
                        let position_offset = last_positions_iter as i64 - input.relative_position() as i64;
                        focus_positions = positional_intersect(&focus_positions,
                                                               &v.positions(),
                                                               (position_offset, position_offset))
                            .iter()
                            .map(|pos| pos.1)
                            .collect::<Vec<_>>();
                        if focus_positions.is_empty() {
                            // No suitable positions found. Next document
                            v = try_option!(input.next());
                            focus = Some(v);
                            focus_positions = v.1.clone();
                            last_doc_iter = i;
                            last_positions_iter = input.relative_position();
                            continue 'possible_documents;
                        } else {
                            last_positions_iter = input.relative_position();
                            break 'term_documents;
                        }
                    } else {
                        // If it is larger, we are now looking at a different focus.
                        // Reset focus and last_iter. Then start from the beginning
                        focus = Some(v);
                        focus_positions = v.1.clone();
                        last_doc_iter = i;
                        last_positions_iter = input.relative_position();
                        continue 'possible_documents;
                    }
                }
            }
            return focus;
        }
    }

    fn next_or(&self) -> Option<&Posting> {

        // Find the smallest current value of all operands
        let min_value = self.operands
            .iter()
            .map(|op| op.peek())
            .filter(|val| val.is_some())
            .map(|val| val.unwrap().0)
            .min();
        // If there is such a value
        if let Some(min) = min_value {
            let mut tmp = None;
            let mut i = 0;
            // Loop over all operands. Advance the ones which currently yield that minimal
            // value
            // Throw the ones out which are empty. Then return the minimal value as
            // reference
            while i < self.operands.len() {
                if let Some(val) = self.operands[i].peek() {
                    if val.0 == min {
                        tmp = self.operands[i].next();
                    }
                }
                i += 1;
            }
            return tmp;
        } else {
            return None;
        }
    }

    fn next_and(&self) -> Option<&Posting> {
        let mut focus = try_option!(self.operands[0].next()); //Acts as temporary to be compared against
        let mut last_iter = 0; //The iterator that last set 'focus'
        'possible_documents: loop {
            // For every term
            for (i, input) in self.operands.iter().enumerate() {
                // If the focus was set by the current iterator, we have a match
                // We cycled through all the iterators once
                if last_iter == i {
                    continue;
                }
                let v = try_option!(input.next_seek(focus));
                if v.0 > focus.0 {
                    // If it is larger, we are now looking at a different focus.
                    // Reset focus and last_iter. Then start from the beginning
                    focus = v;
                    last_iter = i;
                    continue 'possible_documents;
                }
            }
            return Some(focus);
        }
    }
}


pub fn positional_intersect(lhs: &[u32], rhs: &[u32], bounds: (i64, i64)) -> Vec<(u32, u32)> {

    // To understand this algorithm imagine a table.
    // The columns are headed with the values from the rhs slice
    // The rows start with the values from the lhs slice
    // The value in each cell is its row value minus its column value
    // Example:

    // |   | 0 |  4 |  5 |  7 |
    // | 1 | 1 | -3 | -4 | -6 |
    // | 3 | 3 | -1 | -2 | -4 |
    // | 4 | 4 |  0 | -1 | -3 |
    // | 8 | 8 |  4 |  3 |  1 |

    // As both rhs and lhs are sorted we can assume two things:
    // 1. if we "go down" the result of the substraction is going to grow
    // 2. if we "go right" the result of the substraction is going to shrink

    // This algorithm walks through this table. If a difference is to great it will
    // "go right"
    // Otherwise it will go down.
    // If a difference is inside the bounds it will check
    // to the left and to the right for adjacent matches

    let mut result = Vec::new();

    let mut lhs_ptr = 0;
    let mut rhs_ptr = 0;

    while lhs_ptr < lhs.len() && rhs_ptr < rhs.len() {
        let (lval, rval) = (lhs[lhs_ptr] as i64, rhs[rhs_ptr] as i64);
        let diff = lval - rval;
        if diff >= bounds.0 && diff <= bounds.1 {
            result.push((lhs[lhs_ptr], rhs[rhs_ptr]));

            // check "downwards"
            let mut d = lhs_ptr + 1;
            while d < lhs.len() && lhs[d] as i64 - rval <= bounds.1 {
                result.push((lhs[d], rhs[rhs_ptr]));
                d += 1;
            }

            // check "right"
            let mut r = rhs_ptr + 1;
            while r < rhs.len() && lval - rhs[r] as i64 >= bounds.0 {
                result.push((lhs[lhs_ptr], rhs[r]));
                r += 1;
            }

            rhs_ptr += 1;
            lhs_ptr += 1;
            continue;
        }
        if diff >= bounds.1 {
            rhs_ptr += 1;
        }
        if diff <= bounds.0 {
            lhs_ptr += 1;
        }
    }
    result
}
