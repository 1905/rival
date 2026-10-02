mod command_antislop;
mod command_plan;
mod command_security;
mod detach;
mod gitscope_helper;
mod gocsv;
mod install;
mod merge_request;
mod model_command;
mod model_run;
mod model_specs;
mod queue_sessions;
mod root;
mod signals;
mod startup_fds;
#[cfg(test)]
mod testutil;
mod tree;
// Task 4.5 wires the dashboard to the root command; until then only its
// tests use it.
#[allow(dead_code)]
mod tui;
mod update_cmd;
mod wait;
mod workdir;

fn main() {
    std::process::exit(root::main_entry());
}
