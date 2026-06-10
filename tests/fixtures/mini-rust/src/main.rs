mod config;
mod io;
mod processor;

fn main() {
    let config = config::Config::from_env();

    if config.verbose {
        println!("Starting with max_items={}", config.max_items);
    }

    let items: Vec<processor::Item> = (0..10)
        .map(|i| processor::Item {
            id: i,
            data: format!("item_{}", i),
        })
        .collect();

    let filtered = processor::filter_items(items, 6);
    let transformed = processor::transform_items(filtered);

    for s in &transformed {
        println!("{}", s);
    }
}