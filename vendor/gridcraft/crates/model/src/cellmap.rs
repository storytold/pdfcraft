//! Serde helpers for maps keyed by `CellRef`. JSON object keys must be strings, so the keys are
//! written as `A1`-style text.

use std::collections::BTreeMap;

use gridcraft_core::CellRef;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub fn serialize<S: Serializer, V: Serialize>(map: &BTreeMap<CellRef, V>, s: S) -> Result<S::Ok, S::Error> {
    s.collect_map(map.iter().map(|(k, v)| (k.a1(), v)))
}

pub fn deserialize<'de, D: Deserializer<'de>, V: Deserialize<'de>>(d: D) -> Result<BTreeMap<CellRef, V>, D::Error> {
    let raw = BTreeMap::<String, V>::deserialize(d)?;
    let mut out = BTreeMap::new();
    for (k, v) in raw {
        let cell = CellRef::parse(&k).ok_or_else(|| D::Error::custom(format!("invalid cell reference `{k}`")))?;
        out.insert(cell, v);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::{Comment, Hyperlink, Sheet};
    use gridcraft_core::CellRef;

    #[test]
    fn comments_and_hyperlinks_survive_json() -> Result<(), Box<dyn std::error::Error>> {
        let mut s = Sheet::new("S");
        let a1 = CellRef::new(0, 0);
        let c3 = CellRef::new(2, 2);
        s.comments.insert(
            a1,
            Comment {
                author: "Example".into(),
                text: "note".into(),
                replies: vec![("B".into(), "r".into())],
                threaded: true,
                resolved: false,
                visible: false,
                dates: vec![],
                note_box: None,
            },
        );
        s.hyperlinks.insert(c3, Hyperlink { target: "Sheet1!A1".into(), tooltip: Some("jump".into()), external: false });
        let json = serde_json::to_string(&s)?;
        let back: Sheet = serde_json::from_str(&json)?;
        assert_eq!(back.comments, s.comments);
        assert_eq!(back.hyperlinks, s.hyperlinks);
        Ok(())
    }
}
