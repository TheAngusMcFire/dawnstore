use clap::Parser;

mod args;
mod config;

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
            println!("{:20} {:20} {:20}", "Kind:", "ApiVersion:", "Aliases:");
            println!("------------------------------------------------------");
            let rd = api.get_resource_definitions(&Default::default()).await?;
            for r in rd {
                println!(
                    "{:20} {:20} {:20}",
                    r.kind,
                    r.api_version,
                    r.aliases.join(", ")
                );
            }
        }
        args::Commands::Get { resource } => {}
        args::Commands::Delete {
            resource,
            item_name,
        } => todo!(),
        args::Commands::Edit {
            resource,
            item_name,
        } => todo!(),
        args::Commands::Apply { path } => todo!(),
    }
    Ok(())
}
