use clap::Parser;

mod args;

fn main() {
    let args = args::Cli::parse();
    dbg!(&args);
    println!("Hello, world!");
}
