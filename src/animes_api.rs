use serde::Deserialize;
use serde::Serialize;
use std::fmt::Display;

pub type FullAnimeslist = Vec<Root2>;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Root2 {
    pub id: f64,
    #[serde(rename = "source_url")]
    pub source_url: String,
    pub banner: Option<String>,
    pub affiche: String,
    pub title_o: String,
    pub title: String,
    pub titles: Titles,
    pub description: String,
    pub note: String,
    pub themes: Vec<String>,
    pub format: String,
    pub start_date: String,
    pub end_date: Option<String>,
    pub status: String,
    pub nsfw: bool,
    pub saisons: Vec<Saison>,
    #[serde(rename = "affiche_small")]
    pub affiche_small: Option<String>,
    pub updated_date: Option<i64>,
    #[serde(rename = "updatedDateVF")]
    pub updated_date_vf: Option<i64>,
    #[serde(rename = "banner_small")]
    pub banner_small: Option<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Titles {
    #[serde(rename = "en_jp")]
    pub en_jp: Option<String>,
    #[serde(rename = "en_us")]
    pub en_us: Option<String>,
    #[serde(rename = "ja_jp")]
    pub ja_jp: Option<String>,
    pub en: Option<String>,
    #[serde(rename = "abrv_0")]
    pub abrv_0: Option<String>,
    #[serde(rename = "abrv_1")]
    pub abrv_1: Option<String>,
    #[serde(rename = "abrv_2")]
    pub abrv_2: Option<String>,
    #[serde(rename = "abrv_3")]
    pub abrv_3: Option<String>,
    #[serde(rename = "abrv_4")]
    pub abrv_4: Option<String>,
    #[serde(rename = "abrv_5")]
    pub abrv_5: Option<String>,
    #[serde(rename = "abrv_6")]
    pub abrv_6: Option<String>,
    #[serde(rename = "abrv_7")]
    pub abrv_7: Option<String>,
    #[serde(rename = "abrv_8")]
    pub abrv_8: Option<String>,
    #[serde(rename = "abrv_9")]
    pub abrv_9: Option<String>,
    #[serde(rename = "abrv_10")]
    pub abrv_10: Option<String>,
    #[serde(rename = "abrv_11")]
    pub abrv_11: Option<String>,
    #[serde(rename = "abrv_12")]
    pub abrv_12: Option<String>,
    #[serde(rename = "en_cn")]
    pub en_cn: Option<String>,
    #[serde(rename = "zh_cn")]
    pub zh_cn: Option<String>,
    #[serde(rename = "es_es")]
    pub es_es: Option<String>,
    #[serde(rename = "fr_fr")]
    pub fr_fr: Option<String>,
    #[serde(rename = "en_kr")]
    pub en_kr: Option<String>,
    #[serde(rename = "ko_kr")]
    pub ko_kr: Option<String>,
    #[serde(rename = "th_th")]
    pub th_th: Option<String>,
    pub ar: Option<String>,
    #[serde(rename = "ca_es")]
    pub ca_es: Option<String>,
    #[serde(rename = "da_dk")]
    pub da_dk: Option<String>,
    #[serde(rename = "de_de")]
    pub de_de: Option<String>,
    #[serde(rename = "en_ar")]
    pub en_ar: Option<String>,
    #[serde(rename = "fi_fi")]
    pub fi_fi: Option<String>,
    #[serde(rename = "id_id")]
    pub id_id: Option<String>,
    #[serde(rename = "pl_pl")]
    pub pl_pl: Option<String>,
    #[serde(rename = "pt_pt")]
    pub pt_pt: Option<String>,
    #[serde(rename = "ro_ro")]
    pub ro_ro: Option<String>,
    #[serde(rename = "ru_ru")]
    pub ru_ru: Option<String>,
    #[serde(rename = "sv_se")]
    pub sv_se: Option<String>,
    #[serde(rename = "vi_vn")]
    pub vi_vn: Option<String>,
    #[serde(rename = "zh_tw")]
    pub zh_tw: Option<String>,
    pub fr: Option<String>,
    #[serde(rename = "it_it")]
    pub it_it: Option<String>,
    #[serde(rename = "en_fr")]
    pub en_fr: Option<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Saison {
    pub title: String,
    pub episodes: Vec<Episode>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    pub title: String,
    pub lang: Lang,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lang {
    pub vf: Vf,
    pub vo: Vo,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vf {
    pub lecteurs: Vec<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Vo {
    pub lecteurs: Vec<String>,
}
