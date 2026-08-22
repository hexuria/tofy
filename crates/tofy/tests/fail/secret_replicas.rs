use tofy::prelude::*;

fn main() {
    let _ = secret("signing").replicas(2);
}
