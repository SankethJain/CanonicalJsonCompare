//! Generates a pair of realistic sample exports so people can try the tool
//! without real data (`mongo-compare --demo`).

use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*: tiny, deterministic, good enough for sample data.
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(1000) < percent * 10
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len() as u64) as usize]
    }
}

const CITIES: &[&str] = &[
    "Amsterdam",
    "Berlin",
    "Chicago",
    "Dublin",
    "Lisbon",
    "Madrid",
    "Oslo",
    "Paris",
    "Toronto",
];
const STATUSES: &[&str] = &["new", "paid", "shipped", "delivered", "returned"];
const PRODUCTS: &[&str] = &[
    "Desk lamp",
    "Notebook",
    "Backpack",
    "Coffee mug",
    "Headphones",
    "Water bottle",
];
const NAMES: &[&str] = &[
    "Ana", "Ben", "Chen", "Dana", "Eli", "Fatima", "Gus", "Hana", "Ivan", "Jo",
];

fn date(ms: i64) -> Value {
    json!({"$date": {"$numberLong": ms.to_string()}})
}

fn make_doc(rng: &mut Rng, i: u64) -> Value {
    // Orders spread between 2024-01-01 and 2024-06-30 (UTC).
    let start = 1_704_067_200_000i64;
    let created = start + rng.below(181 * 24 * 3600) as i64 * 1000;
    let updated = created + rng.below(20 * 24 * 3600) as i64 * 1000;
    let oid = format!(
        "{:08x}{:016x}",
        created / 1000,
        0xa1b2_c3d4_0000_0000u64 + i
    );
    let items: Vec<Value> = (0..1 + rng.below(3))
        .map(|_| {
            json!({
                "product": rng.pick(PRODUCTS),
                "qty": {"$numberInt": (1 + rng.below(4)).to_string()},
                "price": {"$numberDouble": format!("{}.{:02}", 5 + rng.below(90), rng.below(100))},
            })
        })
        .collect();
    json!({
        "_id": {"$oid": oid},
        "orderNumber": {"$numberLong": (100_000 + i).to_string()},
        "customer": {
            "name": rng.pick(NAMES),
            "address": {"city": rng.pick(CITIES), "zip": format!("{:05}", rng.below(99_999))},
        },
        "status": rng.pick(STATUSES),
        "items": items,
        "giftWrap": rng.chance(20),
        "createdAt": date(created),
        "updatedAt": date(updated),
    })
}

/// Applies the kind of drift a broken migration or sync job typically causes.
fn mutate(rng: &mut Rng, doc: &mut Value) -> bool {
    let roll = rng.below(1000);
    let obj = doc.as_object_mut().unwrap();
    match roll {
        0..=14 => {
            obj["status"] = json!(rng.pick(STATUSES).to_uppercase());
        }
        15..=24 => {
            obj["customer"]["address"]["city"] = json!(rng.pick(CITIES));
        }
        25..=31 => {
            // Number exported as text: a classic type drift.
            let qty = obj["items"][0]["qty"]["$numberInt"]
                .as_str()
                .unwrap_or("1")
                .to_string();
            obj["items"][0]["qty"] = json!(qty);
        }
        32..=37 => {
            obj.remove("giftWrap");
        }
        38..=42 => {
            obj.insert("legacyFlag".into(), json!(true));
        }
        43..=46 => {
            if let Some(items) = obj["items"].as_array_mut() {
                items.push(json!({"product": "Gift card", "qty": {"$numberInt": "1"}, "price": {"$numberDouble": "25.00"}}));
            }
        }
        _ => return false,
    }
    // A changed document usually also gets a new updatedAt.
    if let Some(ms) = crate::extjson::date_millis(&obj["updatedAt"]) {
        obj["updatedAt"] = date(ms + 3_600_000 * (1 + rng.below(48) as i64));
    }
    true
}

pub fn generate(dir: &Path, count: u64) -> io::Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(dir)?;
    let src_path = dir.join("demo-source.json");
    let dst_path = dir.join("demo-destination.json");
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    let docs: Vec<Value> = (0..count).map(|i| make_doc(&mut rng, i)).collect();

    let mut src = BufWriter::new(std::fs::File::create(&src_path)?);
    for d in &docs {
        serde_json::to_writer(&mut src, d)?;
        writeln!(src)?;
    }
    src.flush()?;

    // The destination is in a different order, a few objects are missing,
    // some were changed and a handful of new ones were added.
    let mut dest: Vec<Value> = Vec::with_capacity(docs.len());
    for d in &docs {
        if rng.chance(1) {
            continue;
        }
        let mut copy = d.clone();
        mutate(&mut rng, &mut copy);
        dest.push(copy);
    }
    for i in 0..12 {
        dest.push(make_doc(&mut rng, count + i));
    }
    for i in (1..dest.len()).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        dest.swap(i, j);
    }
    let mut dst = BufWriter::new(std::fs::File::create(&dst_path)?);
    for d in &dest {
        serde_json::to_writer(&mut dst, d)?;
        writeln!(dst)?;
    }
    dst.flush()?;
    Ok((src_path, dst_path))
}
