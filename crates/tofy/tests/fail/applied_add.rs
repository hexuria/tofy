use tofy::prelude::*;

fn main() {
    let extra = postgres("other");
    let applied = stack("d").add(postgres("x")).apply();
    applied.add(extra);
}
