use std::collections::{BTreeMap, HashMap};

pub(super) struct Arrivals<T> {
    order: BTreeMap<u64, (String, T)>,
    places: HashMap<String, u64>,
    arrived: u64,
}

impl<T> Default for Arrivals<T> {
    fn default() -> Self {
        Self {
            order: BTreeMap::new(),
            places: HashMap::new(),
            arrived: 0,
        }
    }
}

impl<T> Arrivals<T> {
    pub(super) fn insert(&mut self, key: String, value: T) {
        self.arrived += 1;
        if let Some(old) = self.places.insert(key.clone(), self.arrived) {
            self.order.remove(&old);
        }
        self.order.insert(self.arrived, (key, value));
    }

    pub(super) fn remove(&mut self, key: &str) -> Option<T> {
        let place = self.places.remove(key)?;
        self.order.remove(&place).map(|(_, value)| value)
    }

    pub(super) fn first(&self) -> Option<&T> {
        self.order.values().next().map(|(_, value)| value)
    }

    pub(super) fn retain(&mut self, mut keeps: impl FnMut(&str) -> bool) {
        self.take(|key| !keeps(key));
    }

    pub(super) fn take(&mut self, mut takes: impl FnMut(&str) -> bool) -> Vec<T> {
        let places = &mut self.places;
        self.order
            .extract_if(.., |_, (key, _)| takes(key.as_str()))
            .map(|(_, (key, value))| {
                places.remove(&key);
                value
            })
            .collect()
    }

    pub(super) fn take_while(&mut self, mut takes: impl FnMut(&T) -> bool) -> Vec<T> {
        let mut taken = Vec::new();
        while let Some(entry) = self.order.first_entry() {
            if !takes(&entry.get().1) {
                break;
            }
            let (key, value) = entry.remove();
            self.places.remove(&key);
            taken.push(value);
        }
        taken
    }

    pub(super) fn clear(&mut self) {
        self.order.clear();
        self.places.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::Arrivals;

    fn filled(keys: &[&str]) -> Arrivals<String> {
        let mut arrivals = Arrivals::default();
        for key in keys {
            arrivals.insert((*key).to_owned(), (*key).to_owned());
        }
        arrivals
    }

    #[test]
    fn a_key_inserted_again_moves_to_the_end_with_its_new_value() {
        let mut arrivals = filled(&["a", "b", "c"]);

        arrivals.insert("a".to_owned(), "a2".to_owned());

        assert_eq!(arrivals.take(|_| true), ["b", "c", "a2"]);
    }

    #[test]
    fn taking_or_removing_a_key_forgets_it_so_inserting_it_again_adds_one_entry() {
        let mut arrivals = filled(&["a", "b", "c"]);

        assert_eq!(arrivals.take(|key| key == "b"), ["b"]);
        assert_eq!(arrivals.remove("a"), Some("a".to_owned()));
        arrivals.retain(|key| key != "c");
        arrivals.insert("b".to_owned(), "b2".to_owned());
        arrivals.insert("a".to_owned(), "a2".to_owned());

        assert_eq!(arrivals.take(|_| true), ["b2", "a2"]);
    }

    #[test]
    fn taking_while_a_value_qualifies_stops_at_the_first_that_does_not() {
        let mut arrivals = filled(&["a", "b", "c"]);

        assert_eq!(arrivals.take_while(|value| value != "b"), ["a"]);
        assert_eq!(arrivals.first().map(String::as_str), Some("b"));
    }
}
