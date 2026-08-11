fn main() {
    let mut version = 0;
    unsafe { epanet_sys::EN_getversion(&mut version) };
    println!("epanet-sys toolkit version {version}");
}
