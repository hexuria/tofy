use tofy::prelude::*;

#[tofy::main]
fn main() {
    let db = postgres("appdb").version("16").port(5433).size(Size::Small);
    let cache = redis("cache");
    let files = bucket("uploads");
    stack("demo").add(db).add(cache).add(files);
}
