/// 将字节数格式化为易读的大小字符串（B, KB, MB, GB）
/// 
/// # 示例
/// ```
/// assert_eq!(format_file_size(512), "512B");
/// assert_eq!(format_file_size(1500), "1.5KB");
/// assert_eq!(format_file_size(1_500_000), "1.5MB");
/// assert_eq!(format_file_size(1_500_000_000), "1.5GB");
/// ```
pub fn format_file_size(bytes: usize) -> String {
    const KB: usize = 1000;
    const MB: usize = 1000 * 1000;
    const GB: usize = 1000 * 1000 * 1000;

    if bytes >= GB {
        let gb = bytes as f64 / GB as f64;
        format!("{:.1}GB", gb)
    } else if bytes >= MB {
        let mb = bytes as f64 / MB as f64;
        format!("{:.1}MB", mb)
    } else if bytes >= KB {
        let kb = bytes as f64 / KB as f64;
        format!("{:.1}KB", kb)
    } else {
        format!("{}B", bytes)
    }
}

/// 将文件名和大小组合成格式化字符串
pub fn format_file_info(filename: &str, bytes: usize) -> String {
    let size_str = format_file_size(bytes);
    if filename.is_empty() {
        size_str
    } else {
        format!("{} ({})", filename, size_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_file_size() {
        assert_eq!(format_file_size(512), "512B");
        assert_eq!(format_file_size(1000), "1.0KB");
        assert_eq!(format_file_size(1500), "1.5KB");
        assert_eq!(format_file_size(1_000_000), "1.0MB");
        assert_eq!(format_file_size(1_500_000), "1.5MB");
        assert_eq!(format_file_size(1_000_000_000), "1.0GB");
        assert_eq!(format_file_size(1_500_000_000), "1.5GB");
    }

    #[test]
    fn test_format_file_info() {
        assert_eq!(format_file_info("", 512), "512B");
        assert_eq!(format_file_info("image.png", 1500), "image.png (1.5KB)");
        assert_eq!(format_file_info("video.mp4", 1_500_000), "video.mp4 (1.5MB)");
    }
}
