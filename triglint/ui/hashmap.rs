// HashMap's default hasher (RandomState) seeds SipHash from OS randomness,
// so iteration order differs between runs. The constructor call edges are
// inlined away inside std's shipped MIR, so this is caught structurally: the
// hasher type appears in the generic arguments of every map operation.

use std::collections::HashMap;

fn main() {
    let mut m: HashMap<u32, u32> = HashMap::new();
    m.insert(1, 2);
    for (k, v) in &m {
        let _ = (k, v);
    }
}
