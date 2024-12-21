mod task;
mod utils;
mod runtime;
pub mod macros;
mod driver;
mod fs;

pub fn add(left: u64, right: u64) -> u64 {
    left + right
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let mut rt = runtime::builder::RuntimeBuilder::<driver::IoUringDriver>::new()
            .build()
            .unwrap();
        rt.block_on(async {
            runtime::runtime::spawn(async  {
                println!("it works 0");
            });
            println!("it works1!")
        });

    }
}
