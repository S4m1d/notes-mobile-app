fn main() {
    // Icons must live inside the binary: there is no asset directory on the phone.
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles);
    slint_build::compile_with_config("ui/app.slint", config).expect("slint compilation failed");
}
