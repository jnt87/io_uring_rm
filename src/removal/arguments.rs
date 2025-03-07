use clap::{Parser};

#[derive(Parser, Default,Debug)]
#[command(name = "rm", about = "removal in chunks")]
pub struct Arguments {
    pub root: String,
    #[arg(short = 'c', long = "confirm", default_value_t = false)]
    pub confirm: bool,
    #[arg(short = 'b', long = "batch_size", default_value_t = 5)]
    pub batch_size: usize,
    #[arg(short = 't', long = "testing", default_value_t = false)]
    pub testing: bool,
}
