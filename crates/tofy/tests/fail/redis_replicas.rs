use tofy::prelude::*;

fn main() {
    let _ = redis("x").replicas(2);
}
