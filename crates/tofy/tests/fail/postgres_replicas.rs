use tofy::prelude::*;

fn main() {
    let _ = postgres("x").replicas(2);
}
