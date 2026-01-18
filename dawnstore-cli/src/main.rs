use clap::Parser;
use color_eyre::eyre::bail;
use dawnstore_lib::*;

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
            let mut filter = GetObjectsFilter {
                namespace: if args.all_namespaces {
                    None
                } else {
                    Some(args.namespace.as_deref().unwrap_or("default").to_string())
                },
                kind: Some(resource.clone()),
                name: Some(item_name.clone()),
                page: None,
                page_size: None,
            };
            let mut rd = api.get_objects(&filter).await?;
            let Some(obj) = rd.pop() else {
                bail!("not found");
            };
            let yaml_file = serde_yml::to_string(&obj)?;
            let Some(x) = utils::edit_with_default_editor(yaml_file.as_str())? else {
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
