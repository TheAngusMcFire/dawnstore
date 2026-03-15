use std::io::Write;

use clap::Parser;
use color_eyre::eyre::{OptionExt, bail};
use dawnstore_lib::*;
use serde_json::Value;
use tempfile::NamedTempFile;

mod args;
mod config;
mod utils;

/// Convert an API error to an eyre error using the Debug representation so
/// that structured enum variants (e.g. `ServerError(UnknownResourceKind {...})`)
/// are visible in the output instead of the flattened Display string.
fn api_err(e: impl std::fmt::Debug) -> color_eyre::eyre::Error {
    color_eyre::eyre::eyre!("{e:?}")
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    // Handle `completions` before full parsing so --context-path isn't required.
    if std::env::args().any(|a| a == "completions") {
        args::Commands::print_completions();
        return Ok(());
    }

    let args = args::Cli::parse();
    let file = std::fs::read_to_string(&args.context_path)?;
    let context = serde_yml::from_str::<config::Context>(&file)?;

    // Token priority: --token / DAWNSTORE_TOKEN env var > context file token field.
    let token = args.token.or(context.token);
    let api = match token {
        Some(t) => dawnstore_client_lib::Api::new_with_token(&context.url, t),
        None => dawnstore_client_lib::Api::new(&context.url),
    };

    let ns = args.namespace.as_deref().unwrap_or("default");

    match &args.command {
        args::Commands::Create {
            resource_kind:
                args::CreationKind::Token {
                    token_name,
                    service_account,
                },
        } => {
            let token = api
                .issue_service_account_token(&IssueTokenRequest {
                    namespace: ns.to_string(),
                    service_account: service_account.clone(),
                    token_name: token_name.clone(),
                    expires_at: None,
                })
                .await?;
            println!("{}", token.token);
        }
        args::Commands::Create {
            resource_kind: args::CreationKind::Namespace { name },
        } => {
            // Namespaces must always be created inside "system".
            let obj = dawnstore_client_lib::Object {
                namespace: Some("system".to_string()),
                api_version: Some("v1".to_string()),
                kind: Some("namespace".to_string()),
                name: name.clone(),
                spec: serde_json::Value::Object(Default::default()),
                id: None,
                created_at: None,
                updated_at: None,
                annotations: None,
                labels: None,
            };
            api.apply(&obj).await.map_err(api_err)?;
            println!("namespace/{name} created");
        }
        args::Commands::Create {
            resource_kind: args::CreationKind::ServiceAccount { name },
        } => {
            let obj = dawnstore_client_lib::Object {
                namespace: Some(ns.to_string()),
                api_version: Some("v1".to_string()),
                kind: Some("serviceaccount".to_string()),
                name: name.clone(),
                spec: serde_json::Value::Object(Default::default()),
                id: None,
                created_at: None,
                updated_at: None,
                annotations: None,
                labels: None,
            };
            api.apply(&obj).await.map_err(api_err)?;
            println!("serviceaccount/{name} created");
        }
        args::Commands::Get { resource }
            if resource == "resource-definitions" || resource == "rd" =>
        {
            let rd = api
                .get_resource_definitions(&Default::default())
                .await
                .map_err(api_err)?;
            println!("{:20} {:20} {:20}", "Kind:", "ApiVersion:", "Aliases:");
            println!("------------------------------------------------------");
            for r in rd {
                println!(
                    "{:20} {:20} {:20}",
                    r.kind,
                    r.api_version,
                    r.aliases.join(", ")
                );
            }
        }
        args::Commands::Get { resource } => {
            let filter = GetObjectsFilter {
                namespace: if args.all_namespaces {
                    None
                } else {
                    Some(args.namespace.as_deref().unwrap_or("default").to_string())
                },
                kind: if resource == "all" {
                    None
                } else {
                    Some(resource.clone())
                },
                ..Default::default()
            };
            let rd = api.get_objects(&filter).await.map_err(api_err)?;
            println!(
                "{:20} {:20} {:20} {:10} {:20}",
                "Namespace:", "Name:", "Kind:", "ApiVersion:", "Created:"
            );
            println!(
                "----------------------------------------------------------------------------------------------------------------------"
            );
            for r in rd {
                println!(
                    "{:20} {:20} {:20} {:10} {:20}",
                    r.namespace, r.name, r.kind, r.api_version, r.created_at
                );
            }
        }
        args::Commands::Delete { resource, item_name } => {
            api.delete_object(&DeleteObject {
                namespace: Some(ns.to_string()),
                kind: resource.clone(),
                name: item_name.clone(),
            })
            .await
            .map_err(api_err)?;
            println!("deleted {resource}/{item_name}");
        }
        args::Commands::Edit {
            resource,
            item_name,
        } => {
            let filter = GetObjectsFilter {
                namespace: Some(args.namespace.as_deref().unwrap_or("default").to_string()),
                kind: Some(resource.clone()),
                name: Some(item_name.clone()),
                fill_child_foreign_keys: true,
                fill_parent_foreign_keys: true,
                ..Default::default()
            };
            let mut rd = api.get_objects(&filter).await.map_err(api_err)?;
            let Some(obj) = rd.pop() else {
                bail!("object not found");
            };
            let schema_filter = Default::default();
            let Some(schema) = api
                .get_resource_definitions(&schema_filter)
                .await
                .map_err(api_err)?
                .into_iter()
                .find(|x| x.api_version == obj.api_version && x.kind == obj.kind)
            else {
                bail!("schema not found")
            };
            let mut json_schema_value =
                serde_json::from_str::<serde_json::Value>(&schema.json_schema)?;
            if let Some(Value::Object(props)) = json_schema_value.get_mut("properties") {
                [
                    "id",
                    "created_at",
                    "updated_at",
                    "namespace",
                    "api_version",
                    "kind",
                    "name",
                ]
                .iter()
                .for_each(|x| {
                    props.insert(
                        x.to_string(),
                        Value::Object(FromIterator::from_iter([(
                            "type".to_string(),
                            Value::String("string".to_string()),
                        )])),
                    );
                });
            }
            let str_json_schema = serde_json::to_string(&json_schema_value)?;
            let mut file = NamedTempFile::with_suffix(".json")?;
            file.write_all(str_json_schema.as_bytes())?;
            file.flush()?;
            let schema_file_name = file.path().to_str().ok_or_eyre("invalid temp path")?;
            let mut whole_file =
                format!("# yaml-language-server: $schema={}\n\n", schema_file_name);
            let yaml_file = serde_yml::to_string(&obj)?;
            whole_file.push_str(&yaml_file);
            let Some(x) = utils::edit_with_default_editor(whole_file.as_str())? else {
                println!("nothing changed");
                return Ok(());
            };
            let value = serde_yml::from_str::<serde_json::Value>(&x)?;
            let json_file = serde_json::to_string(&value)?;
            api.apply_str(json_file).await.map_err(api_err)?;
        }
        args::Commands::Apply { path } => {
            let file = std::fs::read_to_string(path)?;
            let value = serde_yml::from_str::<serde_json::Value>(&file)?;
            let json_file = serde_json::to_string(&value)?;
            api.apply_str(json_file)
                .await
                .map_err(api_err)?
                .iter()
                .for_each(|x| println!("{}", x.name));
        }
        args::Commands::Completions => unreachable!("handled before api setup"),
    }
    Ok(())
}
