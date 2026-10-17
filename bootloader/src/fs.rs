use core::slice;
use uefi::boot::{allocate_pool, image_handle, MemoryType};
use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode};
use uefi::{cstr16, CStr16};

pub fn load_file(path: &CStr16) -> Result<&'static [u8], &'static str> {
    let mut fs = uefi::boot::get_image_file_system(image_handle())
        .map_err(|_| "Failed to get image file system")?;

    let mut root = fs.open_volume()
        .map_err(|_| "Failed to open root volume")?;

    let handle = root.open(path, FileMode::Read, FileAttribute::empty())
        .map_err(|_| "Failed to open file on filesystem")?;

    let mut file = handle.into_regular_file()
        .ok_or("Opened file is not a regular file")?;

    let mut info_buf = [0u8; 512];
    let file_info = file.get_info::<FileInfo>(&mut info_buf)
        .map_err(|_| "Failed to get file info")?;

    let file_size = file_info.file_size() as usize;
    if file_size == 0 {
        return Err("File size is 0 bytes");
    }

    let buf_ptr = allocate_pool(MemoryType::LOADER_DATA, file_size)
        .map_err(|_| "Failed to allocate memory pool for file")?;

    let buf = unsafe { slice::from_raw_parts_mut(buf_ptr.as_ptr(), file_size) };

    let mut total_read = 0;
    while total_read < file_size {
        let read = file.read(&mut buf[total_read..])
            .map_err(|_| "Failed to read file data")?;
        if read == 0 {
            break;
        }
        total_read += read;
    }

    if total_read != file_size {
        return Err("Incomplete file read");
    }

    Ok(buf)
}

pub fn load_kernel_file() -> Result<&'static [u8], &'static str> {
    // Check root directory first, then EFI\BOOT\
    if let Ok(data) = load_file(cstr16!("kernel.elf")) {
        return Ok(data);
    }
    if let Ok(data) = load_file(cstr16!("EFI\\BOOT\\kernel.elf")) {
        return Ok(data);
    }
    Err("Could not find kernel.elf in \\ or \\EFI\\BOOT\\")
}
