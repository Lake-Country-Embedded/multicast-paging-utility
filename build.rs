fn main() {
    #[cfg(feature = "gui")]
    {
        glib_build_tools::compile_resources(
            &["resources"],
            "resources/multicast-paging-utility.gresource.xml",
            "multicast-paging-utility.gresource",
        );
    }
}
