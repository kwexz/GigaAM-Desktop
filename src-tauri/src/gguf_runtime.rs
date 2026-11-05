//! Explicit, checksum-pinned Windows CPU runtime installation.
use std::{io::{Read,Write},path::Path};
use sha2::{Digest,Sha256};
pub fn install(root:&Path)->Result<(),String>{
    if !cfg!(all(windows,target_arch="x86_64")){return Err("Автоустановка runtime пока только Windows x64".into());}
    let directory=root.join("summary-runtime");std::fs::create_dir_all(&directory).map_err(|_|"Каталог runtime недоступен")?;
    let archive=directory.join("runtime.zip.partial");
    let client=reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(300)).build().map_err(|_|"HTTP недоступен")?;
    let mut response=client.get("https://github.com/ggml-org/llama.cpp/releases/download/b12131/llama-b12131-bin-win-cpu-x64.zip").send().map_err(|_|"Runtime недоступен")?;
    if !response.status().is_success(){return Err("Ошибка скачивания runtime".into());}
    let mut file=std::fs::File::create(&archive).map_err(|_|"Не удалось создать архив")?;let mut hash=Sha256::new();let mut total=0;let mut buffer=[0u8;65536];
    loop{let n=response.read(&mut buffer).map_err(|_|"Загрузка runtime прервана")?;if n==0{break;}total+=n;if total>30*1024*1024{return Err("Превышен размер runtime".into());}hash.update(&buffer[..n]);file.write_all(&buffer[..n]).map_err(|_|"Ошибка записи runtime")?;}
    file.sync_all().map_err(|_|"Ошибка сохранения runtime")?;drop(file);
    if format!("{:x}",hash.finalize())!="179599ab3243100ba919a929ea673a11ec0ccdc898d22f21f0a205bf56dcdf68"{return Err("SHA256 runtime не совпадает".into());}
    let status=std::process::Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command","Expand-Archive -LiteralPath $args[0] -DestinationPath $args[1] -Force"]).arg(&archive).arg(&directory).status().map_err(|_|"Не удалось распаковать runtime")?;
    if !status.success(){return Err("Распаковка runtime не завершена".into());}
    let _=std::fs::remove_file(archive);Ok(())
}
