use tofy::prelude::*;

#[tofy::main]
fn main() {
    let db = postgres("appdb")
        .version("16")
        .port(25432)
        .size(Size::Small)
        .bind(Bind::Localhost);
    let cache = redis("cache").port(26379);
    let files = bucket("uploads");
    stack("demoaws")
        .backend(Backend::Aws)
        .add(db)
        .add(cache)
        .add(files)
        .apply();
}
