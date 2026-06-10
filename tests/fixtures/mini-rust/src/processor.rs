pub struct Item {
    pub id: u64,
    pub data: String,
}

pub fn filter_items(items: Vec<Item>, min_len: usize) -> Vec<Item> {
    items.into_iter().filter(|item| item.data.len() >= min_len).collect()
}

pub fn transform_items(items: Vec<Item>) -> Vec<String> {
    items
        .into_iter()
        .map(|item| format!("[{}] {}", item.id, item.data.to_uppercase()))
        .collect()
}

pub fn count_by_prefix(items: &[Item], prefix: &str) -> usize {
    items.iter().filter(|item| item.data.starts_with(prefix)).count()
}