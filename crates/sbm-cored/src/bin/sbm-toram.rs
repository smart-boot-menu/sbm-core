fn main() {
    if sbm_cored::protect_backing_device() {
        println!("backing-protected");
    } else {
        println!("unchanged");
    }
}
