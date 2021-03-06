use std::hash::Hash;
use std::collections::HashMap;


pub struct Hierarchy<T>(HashMap<T, Vec<T>>, Vec<T>);

impl<T: Hash + Eq + Clone> Hierarchy<T> {
    pub fn new() -> Self {
        Hierarchy(HashMap::new(), vec![])
    }

    pub fn add_element(&mut self, term: T, parent: Option<T>) {
        if self.0.contains_key(&term) {
            panic!("Hierarchy element already exists!");
        }

        self.0.insert(term.clone(), vec![]);

        if let Some(parent) = parent {
            if let Some(parent_node) = self.0.get_mut(&parent) {
                parent_node.push(term);
            } else {
                panic!("Added hierarchical elements in wrong order!");
            }
        } else {
            self.1.push(term.clone());
        }
    }

    pub fn get_child_terms(&self, term: &T) -> Option<&[T]> {
        //TODO: Think about that. Is it possible, that we ask for children of a term that does not exist?
        if let Some(node) = self.0.get(term) {
            Some(&node)
        } else {
            None
        }
    }

    pub fn get_root_terms(&self) -> &[T] {
        &self.1
    }
}
