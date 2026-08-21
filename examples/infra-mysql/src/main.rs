use tofy::prelude::*;

#[tofy::main]
fn main() {
    let sql = mysql("appmysql")
        .version("8")
        .port(3308)
        .size(Size::Small)
        .bind(Bind::Localhost);
    stack("demomysql").add(sql).apply();
}
