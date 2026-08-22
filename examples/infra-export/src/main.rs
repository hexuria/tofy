use tofy::prelude::*;

/// Consume-path aliases: the same stack writes `TOFY_*` and the names an app
/// already reads. The `OAG_*` names are an example shape, not a special case.
#[tofy::main]
fn main() {
    let db = postgres("appdb")
        .version("18")
        .port(5452)
        .export("OAG_DATABASE__URL");
    let cache = redis("cache")
        .version("8")
        .port(6399)
        .export("OAG_REDIS__URL");
    let sign = secret("signing").export("OAG_SECURITY__SIGNING_SECRET");
    let kek = secret("kek").export("OAG_SECURITY__CREDENTIAL_KEK");
    stack("oag").add(db).add(cache).add(sign).add(kek).apply();
}
