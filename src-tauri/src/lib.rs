use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use image::imageops::FilterType;
use image::GenericImageView;
use image::ImageFormat;
use imagequant::RGBA;
use oxipng::{Deflater, InFile, Options, OutFile, StripChunks};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

// ============================================================================
// データ構造
// ============================================================================

/// 画像情報を表す構造体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageInfo {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub size: u64,
    pub original_path: String,
}

/// リサイズオプション
#[derive(Debug, Clone, Deserialize)]
pub struct ResizeOptions {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub maintain_aspect_ratio: bool,
}

/// pngquant 圧縮オプション
#[derive(Debug, Clone, Deserialize)]
pub struct QuantOptions {
    pub quality: u8,
}

/// 出力フォーマット
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Png,
    Webp,
}

/// 一括処理オプション
#[derive(Debug, Clone, Deserialize)]
pub struct ProcessOptions {
    // リサイズ設定
    pub resize_enabled: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub maintain_aspect_ratio: bool,
    // pngquant 設定 (PNG のみ)
    pub quantize_enabled: bool,
    pub quality: u8,
    // oxipng 最適化設定 (PNG のみ)
    pub optimize_enabled: bool,
    // 出力先ディレクトリ (None の場合は元ファイルと同じ場所)
    pub output_dir: Option<String>,
    // 出力フォーマット
    pub output_format: OutputFormat,
}

/// 処理結果
#[derive(Debug, Clone, Serialize)]
pub struct ProcessResult {
    pub success: bool,
    pub original_size: u64,
    pub result_size: u64,
    pub output_path: String,
    pub message: String,
}

/// 進捗イベントのペイロード
#[derive(Debug, Clone, Serialize)]
pub struct ProgressPayload {
    pub completed: usize,
    pub total: usize,
    pub current_file: Option<String>,
    pub result: Option<ProcessResult>,
}

// ============================================================================
// ヘルパー関数
// ============================================================================

/// 画像が PNG かどうかを拡張子で判定
fn is_png(path: &PathBuf) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("png"))
        .unwrap_or(false)
}

/// PNG 以外の画像を PNG に変換してバイト列として返す
fn convert_to_png(path: &PathBuf) -> Result<Vec<u8>, String> {
    let img = image::open(path).map_err(|e| format!("画像を開けません: {}", e))?;

    let mut png_data = Cursor::new(Vec::new());
    img.write_to(&mut png_data, ImageFormat::Png)
        .map_err(|e| format!("PNG への変換に失敗: {}", e))?;

    Ok(png_data.into_inner())
}

/// 新しい寸法を計算するヘルパー関数
fn calculate_new_dimensions(
    orig_w: u32,
    orig_h: u32,
    target_w: Option<u32>,
    target_h: Option<u32>,
    maintain_aspect: bool,
) -> (u32, u32) {
    match (target_w, target_h, maintain_aspect) {
        // 両方指定、アスペクト比維持しない
        (Some(w), Some(h), false) => (w, h),

        // 両方指定、アスペクト比維持
        (Some(w), Some(h), true) => {
            let ratio_w = w as f64 / orig_w as f64;
            let ratio_h = h as f64 / orig_h as f64;
            let ratio = ratio_w.min(ratio_h);
            (
                (orig_w as f64 * ratio).round() as u32,
                (orig_h as f64 * ratio).round() as u32,
            )
        }

        // 幅のみ指定
        (Some(w), None, _) => {
            let ratio = w as f64 / orig_w as f64;
            (w, (orig_h as f64 * ratio).round() as u32)
        }

        // 高さのみ指定
        (None, Some(h), _) => {
            let ratio = h as f64 / orig_h as f64;
            ((orig_w as f64 * ratio).round() as u32, h)
        }

        // 何も指定なし
        (None, None, _) => (orig_w, orig_h),
    }
}

// ============================================================================
// Tauri コマンド
// ============================================================================

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// 画像情報を取得する
#[tauri::command]
fn get_image_info(paths: Vec<String>) -> Result<Vec<ImageInfo>, String> {
    let mut results = Vec::new();

    for path_str in &paths {
        let path = PathBuf::from(path_str);

        if !path.exists() {
            continue;
        }

        // 画像を開く
        let img = match image::open(&path) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("{}: 画像を開けません - {}", path_str, e);
                continue;
            }
        };

        let (width, height) = img.dimensions();

        // ファイルサイズを取得
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        results.push(ImageInfo {
            name: path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string(),
            width,
            height,
            size,
            original_path: path_str.clone(),
        });
    }

    Ok(results)
}

/// PNG 最適化 (oxipng)
#[tauri::command]
fn optimize_images(paths: Vec<String>) -> Result<Vec<ProcessResult>, String> {
    let mut options = Options::from_preset(4);
    options.deflater = Deflater::Libdeflater { compression: 12 };
    options.strip = StripChunks::Safe;
    options.optimize_alpha = true;
    options.fast_evaluation = true;

    let mut results = Vec::new();

    for path_str in &paths {
        let path = PathBuf::from(path_str);

        if !path.exists() {
            results.push(ProcessResult {
                success: false,
                original_size: 0,
                result_size: 0,
                output_path: String::new(),
                message: format!("{}: ファイルが存在しません", path_str),
            });
            continue;
        }

        let original_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let parent = path.parent().unwrap_or(std::path::Path::new("."));
        let output_path = parent.join(format!("{}_optimized.png", stem));

        let output = OutFile::Path {
            path: Some(output_path.clone()),
            preserve_attrs: false,
        };

        let result = if is_png(&path) {
            let input = InFile::Path(path.clone());
            oxipng::optimize(&input, &output, &options)
        } else {
            match convert_to_png(&path) {
                Ok(png_data) => {
                    oxipng::optimize_from_memory(&png_data, &options).and_then(|optimized| {
                        fs::write(&output_path, &optimized)
                            .map_err(|e| oxipng::PngError::Other(e.to_string().into()))?;
                        Ok((png_data.len(), optimized.len()))
                    })
                }
                Err(e) => {
                    results.push(ProcessResult {
                        success: false,
                        original_size,
                        result_size: 0,
                        output_path: String::new(),
                        message: format!("{}: {}", path_str, e),
                    });
                    continue;
                }
            }
        };

        match result {
            Ok((_, optimized_size)) => {
                let result_size = fs::metadata(&output_path)
                    .map(|m| m.len())
                    .unwrap_or(optimized_size as u64);
                results.push(ProcessResult {
                    success: true,
                    original_size,
                    result_size,
                    output_path: output_path.to_string_lossy().to_string(),
                    message: format!(
                        "{} → {} bytes ({:.1}% 削減)",
                        original_size,
                        result_size,
                        if original_size > 0 {
                            (1.0 - result_size as f64 / original_size as f64) * 100.0
                        } else {
                            0.0
                        }
                    ),
                });
            }
            Err(e) => {
                results.push(ProcessResult {
                    success: false,
                    original_size,
                    result_size: 0,
                    output_path: String::new(),
                    message: format!("{}: 最適化に失敗しました - {}", path_str, e),
                });
            }
        }
    }

    Ok(results)
}

/// リサイズ処理
#[tauri::command]
fn resize_images(paths: Vec<String>, options: ResizeOptions) -> Result<Vec<ProcessResult>, String> {
    let mut results = Vec::new();

    for path_str in &paths {
        let path = PathBuf::from(path_str);

        if !path.exists() {
            results.push(ProcessResult {
                success: false,
                original_size: 0,
                result_size: 0,
                output_path: String::new(),
                message: format!("{}: ファイルが存在しません", path_str),
            });
            continue;
        }

        let img = match image::open(&path) {
            Ok(i) => i,
            Err(e) => {
                results.push(ProcessResult {
                    success: false,
                    original_size: 0,
                    result_size: 0,
                    output_path: String::new(),
                    message: format!("{}: 画像を開けません - {}", path_str, e),
                });
                continue;
            }
        };

        let original_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let (orig_w, orig_h) = img.dimensions();

        // 新しいサイズを計算
        let (new_width, new_height) = calculate_new_dimensions(
            orig_w,
            orig_h,
            options.width,
            options.height,
            options.maintain_aspect_ratio,
        );

        // リサイズ実行 (Lanczos3 フィルタ使用)
        let resized = img.resize_exact(new_width, new_height, FilterType::Lanczos3);

        // 出力パスを生成
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let parent = path.parent().unwrap_or(std::path::Path::new("."));
        let output_path = parent.join(format!("{}_resized.png", stem));

        // 保存
        match resized.save(&output_path) {
            Ok(_) => {
                let result_size = fs::metadata(&output_path).map(|m| m.len()).unwrap_or(0);
                results.push(ProcessResult {
                    success: true,
                    original_size,
                    result_size,
                    output_path: output_path.to_string_lossy().to_string(),
                    message: format!(
                        "{}x{} → {}x{} にリサイズしました",
                        orig_w, orig_h, new_width, new_height
                    ),
                });
            }
            Err(e) => {
                results.push(ProcessResult {
                    success: false,
                    original_size,
                    result_size: 0,
                    output_path: String::new(),
                    message: format!("保存エラー: {}", e),
                });
            }
        }
    }

    Ok(results)
}

/// pngquant 圧縮 (imagequant)
#[tauri::command]
fn quantize_images(
    paths: Vec<String>,
    options: QuantOptions,
) -> Result<Vec<ProcessResult>, String> {
    let mut results = Vec::new();

    for path_str in &paths {
        let path = PathBuf::from(path_str);

        if !path.exists() {
            results.push(ProcessResult {
                success: false,
                original_size: 0,
                result_size: 0,
                output_path: String::new(),
                message: format!("{}: ファイルが存在しません", path_str),
            });
            continue;
        }

        // 画像を読み込み
        let img = match image::open(&path) {
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                results.push(ProcessResult {
                    success: false,
                    original_size: 0,
                    result_size: 0,
                    output_path: String::new(),
                    message: format!("画像読み込みエラー: {}", e),
                });
                continue;
            }
        };

        let original_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let (width, height) = img.dimensions();

        // RGBA ピクセルデータを取得
        let pixels: Vec<RGBA> = img
            .pixels()
            .map(|p| RGBA::new(p[0], p[1], p[2], p[3]))
            .collect();

        // imagequant で量子化
        let mut attrs = imagequant::new();

        // クオリティ設定 (min, max)
        let min_quality = (options.quality as u32).saturating_sub(10).max(0);
        let max_quality = options.quality as u32;
        if let Err(e) = attrs.set_quality(min_quality as u8, max_quality as u8) {
            results.push(ProcessResult {
                success: false,
                original_size,
                result_size: 0,
                output_path: String::new(),
                message: format!("クオリティ設定エラー: {:?}", e),
            });
            continue;
        }

        let mut liq_image =
            match attrs.new_image(pixels.as_slice(), width as usize, height as usize, 0.0) {
                Ok(img) => img,
                Err(e) => {
                    results.push(ProcessResult {
                        success: false,
                        original_size,
                        result_size: 0,
                        output_path: String::new(),
                        message: format!("画像作成エラー: {:?}", e),
                    });
                    continue;
                }
            };

        let mut quantized = match attrs.quantize(&mut liq_image) {
            Ok(q) => q,
            Err(e) => {
                results.push(ProcessResult {
                    success: false,
                    original_size,
                    result_size: 0,
                    output_path: String::new(),
                    message: format!("量子化エラー: {:?}", e),
                });
                continue;
            }
        };

        let _ = quantized.set_dithering_level(1.0);

        let (palette, indexed_pixels) = match quantized.remapped(&mut liq_image) {
            Ok(result) => result,
            Err(e) => {
                results.push(ProcessResult {
                    success: false,
                    original_size,
                    result_size: 0,
                    output_path: String::new(),
                    message: format!("リマップエラー: {:?}", e),
                });
                continue;
            }
        };

        // 出力パスを生成
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("output");
        let parent = path.parent().unwrap_or(std::path::Path::new("."));
        let output_path = parent.join(format!("{}_quantized.png", stem));

        // lodepng で PNG として保存
        let mut encoder = lodepng::Encoder::new();

        // パレットを設定
        for color in &palette {
            if let Err(e) = encoder.info_raw_mut().palette_add(lodepng::RGBA {
                r: color.r,
                g: color.g,
                b: color.b,
                a: color.a,
            }) {
                results.push(ProcessResult {
                    success: false,
                    original_size,
                    result_size: 0,
                    output_path: String::new(),
                    message: format!("パレット追加エラー: {:?}", e),
                });
                continue;
            }
            if let Err(e) = encoder.info_png_mut().color.palette_add(lodepng::RGBA {
                r: color.r,
                g: color.g,
                b: color.b,
                a: color.a,
            }) {
                results.push(ProcessResult {
                    success: false,
                    original_size,
                    result_size: 0,
                    output_path: String::new(),
                    message: format!("パレット追加エラー: {:?}", e),
                });
                continue;
            }
        }

        encoder.info_raw_mut().colortype = lodepng::ColorType::PALETTE;
        encoder.info_raw_mut().set_bitdepth(8);
        encoder.info_png_mut().color.colortype = lodepng::ColorType::PALETTE;
        encoder.info_png_mut().color.set_bitdepth(8);

        let png_data = match encoder.encode(&indexed_pixels, width as usize, height as usize) {
            Ok(data) => data,
            Err(e) => {
                results.push(ProcessResult {
                    success: false,
                    original_size,
                    result_size: 0,
                    output_path: String::new(),
                    message: format!("PNG エンコードエラー: {:?}", e),
                });
                continue;
            }
        };

        if let Err(e) = fs::write(&output_path, &png_data) {
            results.push(ProcessResult {
                success: false,
                original_size,
                result_size: 0,
                output_path: String::new(),
                message: format!("ファイル書き込みエラー: {}", e),
            });
            continue;
        }

        let result_size = png_data.len() as u64;

        results.push(ProcessResult {
            success: true,
            original_size,
            result_size,
            output_path: output_path.to_string_lossy().to_string(),
            message: format!(
                "クオリティ {} で圧縮: {} → {} bytes ({:.1}% 削減)",
                options.quality,
                original_size,
                result_size,
                if original_size > 0 {
                    (1.0 - result_size as f64 / original_size as f64) * 100.0
                } else {
                    0.0
                }
            ),
        });
    }

    Ok(results)
}

/// 単一画像の処理（並列処理用）
fn process_single_image(path_str: &str, options: &ProcessOptions) -> ProcessResult {
    let path = PathBuf::from(path_str);

    if !path.exists() {
        return ProcessResult {
            success: false,
            original_size: 0,
            result_size: 0,
            output_path: String::new(),
            message: format!("{}: ファイルが存在しません", path_str),
        };
    }

    let original_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    // 出力先ディレクトリを決定
    let output_parent = if let Some(ref out_dir) = options.output_dir {
        let out_path = PathBuf::from(out_dir);
        // ディレクトリが存在しない場合は作成
        if !out_path.exists() {
            if let Err(e) = fs::create_dir_all(&out_path) {
                return ProcessResult {
                    success: false,
                    original_size,
                    result_size: 0,
                    output_path: String::new(),
                    message: format!("出力ディレクトリ作成エラー: {}", e),
                };
            }
        }
        out_path
    } else {
        path.parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf()
    };

    // 画像を読み込み
    let mut img = match image::open(&path) {
        Ok(i) => i,
        Err(e) => {
            return ProcessResult {
                success: false,
                original_size,
                result_size: 0,
                output_path: String::new(),
                message: format!("{}: 画像を開けません - {}", path_str, e),
            };
        }
    };

    let mut process_steps: Vec<String> = Vec::new();
    let (orig_w, orig_h) = img.dimensions();

    // ステップ 1: リサイズ
    if options.resize_enabled && (options.width.is_some() || options.height.is_some()) {
        let (new_width, new_height) = calculate_new_dimensions(
            orig_w,
            orig_h,
            options.width,
            options.height,
            options.maintain_aspect_ratio,
        );
        img = img.resize_exact(new_width, new_height, FilterType::Lanczos3);
        process_steps.push(format!(
            "リサイズ: {}x{} → {}x{}",
            orig_w, orig_h, new_width, new_height
        ));
    }

    // 出力フォーマットに応じて処理を分岐
    let (final_data, extension) = match options.output_format {
        OutputFormat::Png => {
            // PNG 出力: pngquant → oxipng
            let png_data: Vec<u8> = if options.quantize_enabled {
                let rgba_img = img.to_rgba8();
                let (width, height) = rgba_img.dimensions();

                let pixels: Vec<RGBA> = rgba_img
                    .pixels()
                    .map(|p| RGBA::new(p[0], p[1], p[2], p[3]))
                    .collect();

                let mut attrs = imagequant::new();
                let min_quality = (options.quality as u32).saturating_sub(10).max(0);
                let max_quality = options.quality as u32;

                if let Err(e) = attrs.set_quality(min_quality as u8, max_quality as u8) {
                    return ProcessResult {
                        success: false,
                        original_size,
                        result_size: 0,
                        output_path: String::new(),
                        message: format!("クオリティ設定エラー: {:?}", e),
                    };
                }

                let mut liq_image =
                    match attrs.new_image(pixels.as_slice(), width as usize, height as usize, 0.0) {
                        Ok(img) => img,
                        Err(e) => {
                            return ProcessResult {
                                success: false,
                                original_size,
                                result_size: 0,
                                output_path: String::new(),
                                message: format!("imagequant エラー: {:?}", e),
                            };
                        }
                    };

                let mut quantized = match attrs.quantize(&mut liq_image) {
                    Ok(q) => q,
                    Err(e) => {
                        return ProcessResult {
                            success: false,
                            original_size,
                            result_size: 0,
                            output_path: String::new(),
                            message: format!("量子化エラー: {:?}", e),
                        };
                    }
                };

                let _ = quantized.set_dithering_level(1.0);

                let (palette, indexed_pixels) = match quantized.remapped(&mut liq_image) {
                    Ok(result) => result,
                    Err(e) => {
                        return ProcessResult {
                            success: false,
                            original_size,
                            result_size: 0,
                            output_path: String::new(),
                            message: format!("リマップエラー: {:?}", e),
                        };
                    }
                };

                let mut encoder = lodepng::Encoder::new();
                for color in &palette {
                    let _ = encoder.info_raw_mut().palette_add(lodepng::RGBA {
                        r: color.r,
                        g: color.g,
                        b: color.b,
                        a: color.a,
                    });
                    let _ = encoder.info_png_mut().color.palette_add(lodepng::RGBA {
                        r: color.r,
                        g: color.g,
                        b: color.b,
                        a: color.a,
                    });
                }

                encoder.info_raw_mut().colortype = lodepng::ColorType::PALETTE;
                encoder.info_raw_mut().set_bitdepth(8);
                encoder.info_png_mut().color.colortype = lodepng::ColorType::PALETTE;
                encoder.info_png_mut().color.set_bitdepth(8);

                match encoder.encode(&indexed_pixels, width as usize, height as usize) {
                    Ok(data) => {
                        process_steps.push(format!("pngquant: クオリティ {}", options.quality));
                        data
                    }
                    Err(e) => {
                        return ProcessResult {
                            success: false,
                            original_size,
                            result_size: 0,
                            output_path: String::new(),
                            message: format!("PNG エンコードエラー: {:?}", e),
                        };
                    }
                }
            } else {
                // pngquant をスキップする場合は PNG に変換
                let mut cursor = Cursor::new(Vec::new());
                if let Err(e) = img.write_to(&mut cursor, ImageFormat::Png) {
                    return ProcessResult {
                        success: false,
                        original_size,
                        result_size: 0,
                        output_path: String::new(),
                        message: format!("PNG 変換エラー: {}", e),
                    };
                }
                cursor.into_inner()
            };

            // oxipng 最適化
            let optimized_data: Vec<u8> = if options.optimize_enabled {
                let mut oxi_options = Options::from_preset(4);
                oxi_options.deflater = Deflater::Libdeflater { compression: 12 };
                oxi_options.strip = StripChunks::Safe;
                oxi_options.optimize_alpha = true;
                oxi_options.fast_evaluation = true;

                match oxipng::optimize_from_memory(&png_data, &oxi_options) {
                    Ok(optimized) => {
                        process_steps.push("oxipng: 最適化".to_string());
                        optimized
                    }
                    Err(e) => {
                        return ProcessResult {
                            success: false,
                            original_size,
                            result_size: 0,
                            output_path: String::new(),
                            message: format!("oxipng エラー: {}", e),
                        };
                    }
                }
            } else {
                png_data
            };

            (optimized_data, "png")
        }
        OutputFormat::Webp => {
            // WebP 出力
            let rgba_img = img.to_rgba8();
            let (width, height) = rgba_img.dimensions();

            let webp_data = if options.quality >= 100 {
                // ロスレス
                process_steps.push("WebP: ロスレス".to_string());
                let encoder = webp::Encoder::from_rgba(rgba_img.as_raw(), width, height);
                encoder.encode_lossless().to_vec()
            } else {
                // ロッシー
                process_steps.push(format!("WebP: クオリティ {}", options.quality));
                let encoder = webp::Encoder::from_rgba(rgba_img.as_raw(), width, height);
                encoder.encode(options.quality as f32).to_vec()
            };

            (webp_data, "webp")
        }
    };

    // 最終出力ファイル名
    let output_path = output_parent.join(format!("{}_processed.{}", stem, extension));

    if let Err(e) = fs::write(&output_path, &final_data) {
        return ProcessResult {
            success: false,
            original_size,
            result_size: 0,
            output_path: String::new(),
            message: format!("ファイル書き込みエラー: {}", e),
        };
    }

    let result_size = final_data.len() as u64;
    let reduction = if original_size > 0 {
        (1.0 - result_size as f64 / original_size as f64) * 100.0
    } else {
        0.0
    };

    ProcessResult {
        success: true,
        original_size,
        result_size,
        output_path: output_path.to_string_lossy().to_string(),
        message: format!(
            "{} | {} → {} bytes ({:.1}% 削減)",
            process_steps.join(" → "),
            original_size,
            result_size,
            reduction
        ),
    }
}

/// 一括処理: リサイズ → pngquant → oxipng の順で並列実行
/// 別スレッドで実行することでUIをブロックしない
#[tauri::command]
fn process_images(
    app: AppHandle,
    paths: Vec<String>,
    options: ProcessOptions,
) -> Result<Vec<ProcessResult>, String> {
    let total = paths.len();

    // 処理を別スレッドで非同期実行し、結果は完了イベントで通知
    std::thread::spawn(move || {
        let completed = AtomicUsize::new(0);

        // rayon による並列処理
        let _results: Vec<ProcessResult> = paths
            .par_iter()
            .map(|path_str| {
                let result = process_single_image(path_str, &options);

                // 進捗カウント更新
                let current = completed.fetch_add(1, Ordering::SeqCst) + 1;

                // 進捗イベント送信
                let _ = app.emit(
                    "process-progress",
                    ProgressPayload {
                        completed: current,
                        total,
                        current_file: Some(path_str.clone()),
                        result: Some(result.clone()),
                    },
                );

                result
            })
            .collect();

        // 処理完了をイベントで通知
        let _ = app.emit("process-complete", ());
    });

    // すぐに返す（結果はイベントで送信される）
    Ok(vec![])
}

// ============================================================================
// アプリケーションエントリーポイント
// ============================================================================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            optimize_images,
            get_image_info,
            resize_images,
            quantize_images,
            process_images,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
