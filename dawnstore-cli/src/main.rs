use std::io::Write;

use clap::Parser;
use color_eyre::eyre::{OptionExt, bail};
use dawnstore_lib::*;
use serde_json::{Map, Value};
use tempfile::NamedTempFile;

mod args;
mod config;
mod utils;

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    let args = args::Cli::parse();
    let file = std::fs::read_to_string(&args.context_path)?;
    let context = serde_yml::from_str::<config::Context>(&file)?;
    let api = dawnstore_client_lib::Api::new(&context.url);

    match &args.command {
        args::Commands::Get { resource }
            if resource == "resource-definitions" || resource == "rd" =>
        {
            let rd = api.get_resource_definitions(&Default::default()).await?;
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
            let mut filter = GetObjectsFilter {
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
                name: None,
                page: None,
                page_size: None,
            };
            let rd = api.get_objects(&filter).await?;
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
        args::Commands::Delete {
            resource,
            item_name,
        } => todo!(),
        args::Commands::Edit {
            resource,
            item_name,
        } => {
            let filter = GetObjectsFilter {
                namespace: Some(args.namespace.as_deref().unwrap_or("default").to_string()),
                kind: Some(resource.clone()),
                name: Some(item_name.clone()),
                page: None,
                page_size: None,
            };
            let mut rd = api.get_objects(&filter).await?;
            let Some(obj) = rd.pop() else {
                bail!("object not found");
            };
            let schema_filter = Default::default();
            let Some(schema) = api
                .get_resource_definitions(&schema_filter)
                .await?
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
            api.apply_str(json_file).await?;
        }
        args::Commands::Apply { path } => {
            let file = std::fs::read_to_string(path)?;
            let value = serde_yml::from_str::<serde_json::Value>(&file)?;
            let json_file = serde_json::to_string(&value)?;
            api.apply_str(json_file)
                .await?
                .iter()
                .for_each(|x| println!("{}", x.name));
        }
    }
    Ok(())
}
