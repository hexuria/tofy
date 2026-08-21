use tofy::prelude::*;

#[tofy::main]
fn main() {
    let db = postgres("appdb")
        .version("16")
        .port(15433)
        .size(Size::Small)
        .bind(Bind::Localhost);
    let cache = redis("cache").port(16379);
    let files = bucket("uploads").port(19000);
    stack("demotofu")
        .backend(Backend::Tofu)
        .add(db)
        .add(cache)
        .add(files)
        .apply();
}
