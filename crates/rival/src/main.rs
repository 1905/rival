mod detach;
mod gitscope_helper;
mod gocsv;
mod merge_request;
mod model_command;
mod model_run;
mod model_specs;
mod root;
mod signals;
mod startup_fds;
#[cfg(test)]
mod testutil;
mod tree;
mod wait;
mod workdir;

fn main() {
    std::process::exit(root::main_entry());
}
