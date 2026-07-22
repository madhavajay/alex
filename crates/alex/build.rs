fn main() {
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/alex.ico");
        res.set("ProductName", "Alex");
        res.set("FileDescription", "Alex local control plane");
        if let Err(error) = res.compile() {
            println!("cargo:warning=failed to embed Windows resources: {error}");
        }
    }
}
