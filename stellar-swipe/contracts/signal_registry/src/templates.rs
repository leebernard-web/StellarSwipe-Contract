use soroban_sdk::{contracttype, Address, Env, Map, String, Vec};

use crate::types::SignalAction;

pub const MAX_TEMPLATES_PER_PROVIDER: u32 = 5;

#[contracttype]
#[derive(Clone, Debug)]
pub struct SignalTemplate {
    pub asset_pair: String,
    pub action: SignalAction,
    pub risk_rating: u32,
    pub category: String,
    pub default_expiry_hours: u64,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SignalTemplateOverrides {
    pub asset_pair: Option<String>,
    pub action: Option<u32>,
    pub risk_rating: Option<u32>,
    pub category: Option<String>,
    pub expiry_hours: Option<u64>,
    pub price: Option<i128>,
    pub rationale: Option<String>,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct StoredSignalTemplate {
    pub template_id: u32,
    pub template: SignalTemplate,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TemplateError {
    TemplateLimitReached,
    TemplateNotFound,
}

pub fn save_signal_template(
    env: &Env,
    templates: &mut Map<Address, Vec<StoredSignalTemplate>>,
    provider: Address,
    template: SignalTemplate,
) -> Result<u32, TemplateError> {
    let mut provider_templates = templates.get(provider.clone()).unwrap_or(Vec::new(env));
    if provider_templates.len() >= MAX_TEMPLATES_PER_PROVIDER {
        return Err(TemplateError::TemplateLimitReached);
    }

    let template_id = provider_templates.len() + 1;
    provider_templates.push_back(StoredSignalTemplate {
        template_id,
        template,
    });
    templates.set(provider, provider_templates);

    Ok(template_id)
}

pub fn get_signal_template(
    templates: &Map<Address, Vec<StoredSignalTemplate>>,
    provider: Address,
    template_id: u32,
) -> Result<SignalTemplate, TemplateError> {
    let provider_templates = templates
        .get(provider)
        .ok_or(TemplateError::TemplateNotFound)?;

    for i in 0..provider_templates.len() {
        if let Some(stored) = provider_templates.get(i) {
            if stored.template_id == template_id {
                return Ok(stored.template);
            }
        }
    }

    Err(TemplateError::TemplateNotFound)
}

pub fn merge_template(
    template: SignalTemplate,
    overrides: SignalTemplateOverrides,
) -> (String, SignalAction, u64, i128, String) {
    let asset_pair = overrides.asset_pair.unwrap_or(template.asset_pair);
    let action = match overrides.action {
        Some(1) => SignalAction::Sell,
        Some(0) => SignalAction::Buy,
        _ => template.action,
    };
    let expiry_hours = overrides
        .expiry_hours
        .unwrap_or(template.default_expiry_hours);
    let price = overrides.price.unwrap_or(1);
    let rationale = overrides
        .rationale
        .unwrap_or(overrides.category.unwrap_or(template.category));

    (asset_pair, action, expiry_hours, price, rationale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::testutils::Address as _;

    fn sdk_string(env: &Env, value: &str) -> String {
        String::from_str(env, value)
    }

    fn template(env: &Env) -> SignalTemplate {
        SignalTemplate {
            asset_pair: sdk_string(env, "XLM/USDC"),
            action: SignalAction::Buy,
            risk_rating: 2,
            category: sdk_string(env, "momentum"),
            default_expiry_hours: 24,
        }
    }

    #[test]
    fn save_template() {
        let env = Env::default();
        let provider = Address::generate(&env);
        let mut templates = Map::new(&env);

        let template_id =
            save_signal_template(&env, &mut templates, provider.clone(), template(&env)).unwrap();

        assert_eq!(template_id, 1);
        assert_eq!(templates.get(provider).unwrap().len(), 1);
    }

    #[test]
    fn merge_template_values_without_overrides() {
        let env = Env::default();
        let (asset_pair, action, expiry_hours, price, rationale) = merge_template(
            template(&env),
            SignalTemplateOverrides {
                asset_pair: None,
                action: None,
                risk_rating: None,
                category: None,
                expiry_hours: None,
                price: None,
                rationale: None,
            },
        );

        assert_eq!(asset_pair, sdk_string(&env, "XLM/USDC"));
        assert!(matches!(action, SignalAction::Buy));
        assert_eq!(expiry_hours, 24);
        assert_eq!(price, 1);
        assert_eq!(rationale, sdk_string(&env, "momentum"));
    }

    #[test]
    fn override_fields() {
        let env = Env::default();
        let (asset_pair, action, expiry_hours, price, rationale) = merge_template(
            template(&env),
            SignalTemplateOverrides {
                asset_pair: Some(sdk_string(&env, "BTC/USDC")),
                action: Some(1),
                risk_rating: Some(5),
                category: Some(sdk_string(&env, "hedge")),
                expiry_hours: Some(12),
                price: Some(50),
                rationale: Some(sdk_string(&env, "override")),
            },
        );

        assert_eq!(asset_pair, sdk_string(&env, "BTC/USDC"));
        assert!(matches!(action, SignalAction::Sell));
        assert_eq!(expiry_hours, 12);
        assert_eq!(price, 50);
        assert_eq!(rationale, sdk_string(&env, "override"));
    }

    #[test]
    fn template_limit() {
        let env = Env::default();
        let provider = Address::generate(&env);
        let mut templates = Map::new(&env);

        for _ in 0..MAX_TEMPLATES_PER_PROVIDER {
            save_signal_template(&env, &mut templates, provider.clone(), template(&env)).unwrap();
        }

        let result = save_signal_template(&env, &mut templates, provider, template(&env));
        assert_eq!(result, Err(TemplateError::TemplateLimitReached));
    }
extern crate alloc;

use alloc::string::{String as RustString, ToString};
use alloc::vec::Vec as RustVec;
use core::str;
use soroban_sdk::{contracttype, Address, Env, Map, String};

use crate::errors::TemplateError;
use crate::StorageKey;

pub const DEFAULT_TEMPLATE_EXPIRY_HOURS: u32 = 24;
pub const MAX_SIGNAL_RATIONALE_BYTES: u32 = 500;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SignalTemplate {
    pub id: u64,
    pub provider: Address,
    pub name: String,
    pub asset_pair: Option<String>,
    pub action: Option<String>,
    pub rationale_template: String,
    pub default_expiry_hours: u32,
    pub is_public: bool,
    pub use_count: u32,
}

pub fn get_next_template_id(env: &Env) -> u64 {
    let mut counter: u64 = env
        .storage()
        .instance()
        .get(&StorageKey::TemplateCounter)
        .unwrap_or(0);
    counter = counter.checked_add(1).expect("template id overflow");
    env.storage()
        .instance()
        .set(&StorageKey::TemplateCounter, &counter);
    counter
}

pub fn get_templates_map(env: &Env) -> Map<u64, SignalTemplate> {
    env.storage()
        .instance()
        .get(&StorageKey::Templates)
        .unwrap_or(Map::new(env))
}

pub fn store_template(env: &Env, template_id: u64, template: &SignalTemplate) {
    let mut templates = get_templates_map(env);
    templates.set(template_id, template.clone());
    env.storage()
        .instance()
        .set(&StorageKey::Templates, &templates);
}

pub fn get_template(env: &Env, template_id: u64) -> Option<SignalTemplate> {
    let templates = get_templates_map(env);
    templates.get(template_id)
}

pub fn increment_template_use_count(env: &Env, template_id: u64) -> Result<(), TemplateError> {
    let mut templates = get_templates_map(env);
    let mut template = templates
        .get(template_id)
        .ok_or(TemplateError::TemplateNotFound)?;
    template.use_count = template
        .use_count
        .checked_add(1)
        .ok_or(TemplateError::InvalidTemplate)?;
    templates.set(template_id, template);
    env.storage()
        .instance()
        .set(&StorageKey::Templates, &templates);
    Ok(())
}

pub fn set_template_visibility(
    env: &Env,
    provider: &Address,
    template_id: u64,
    is_public: bool,
) -> Result<(), TemplateError> {
    let mut templates = get_templates_map(env);
    let mut template = templates
        .get(template_id)
        .ok_or(TemplateError::TemplateNotFound)?;
    if &template.provider != provider {
        return Err(TemplateError::Unauthorized);
    }
    template.is_public = is_public;
    templates.set(template_id, template);
    env.storage()
        .instance()
        .set(&StorageKey::Templates, &templates);
    Ok(())
}

pub fn replace_variables(
    env: &Env,
    template: &String,
    variables: &Map<String, String>,
) -> Result<String, TemplateError> {
    let template_text = soroban_to_rust_string(template)?;
    let mut out = RustString::new();

    let bytes = template_text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b'}' {
                j += 1;
            }

            if j == bytes.len() {
                out.push('{');
                i += 1;
                continue;
            }

            let key =
                str::from_utf8(&bytes[(i + 1)..j]).map_err(|_| TemplateError::InvalidTemplate)?;
            if key.is_empty() {
                return Err(TemplateError::InvalidTemplate);
            }

            if let Some(value) = get_variable(variables, key)? {
                out.push_str(soroban_to_rust_string(&value)?.as_str());
            } else if key == "date" {
                out.push_str(env.ledger().timestamp().to_string().as_str());
            } else {
                return Err(TemplateError::MissingVariable);
            }

            i = j + 1;
            continue;
        }

        out.push(bytes[i] as char);
        i += 1;
    }

    let mut out_bytes = out.into_bytes();
    if out_bytes.len() > MAX_SIGNAL_RATIONALE_BYTES as usize {
        out_bytes.truncate(MAX_SIGNAL_RATIONALE_BYTES as usize);
    }

    let out_text = str::from_utf8(&out_bytes).map_err(|_| TemplateError::InvalidTemplate)?;
    Ok(String::from_str(env, out_text))
}

pub fn get_variable(
    variables: &Map<String, String>,
    key: &str,
) -> Result<Option<String>, TemplateError> {
    for map_key in variables.keys() {
        let key_text = soroban_to_rust_string(&map_key)?;
        if key_text == key {
            return Ok(variables.get(map_key));
        }
    }
    Ok(None)
}

pub fn parse_action(action_text: &String) -> Result<crate::types::SignalAction, TemplateError> {
    let action = soroban_to_rust_string(action_text)?;
    let lower = action.to_ascii_lowercase();
    match lower.as_str() {
        "buy" => Ok(crate::types::SignalAction::Buy),
        "sell" => Ok(crate::types::SignalAction::Sell),
        _ => Err(TemplateError::InvalidAction),
    }
}

pub fn parse_price(price_text: &String) -> Result<i128, TemplateError> {
    let price_str = soroban_to_rust_string(price_text)?;
    let price = price_str
        .parse::<i128>()
        .map_err(|_| TemplateError::MissingVariable)?;
    if price <= 0 {
        return Err(TemplateError::MissingVariable);
    }
    Ok(price)
}

fn soroban_to_rust_string(value: &String) -> Result<RustString, TemplateError> {
    let bytes = value.clone().to_bytes();
    let mut raw = RustVec::with_capacity(bytes.len() as usize);
    for i in 0..bytes.len() {
        raw.push(bytes.get(i).unwrap());
    }
    let text = str::from_utf8(&raw).map_err(|_| TemplateError::InvalidTemplate)?;
    Ok(text.to_string())
}
