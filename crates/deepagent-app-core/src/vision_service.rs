//! Local system-vision service.
//!
//! Florence-2 is used as a local image-to-text bridge: it converts screenshots
//! and images into text that the normal chat model can read. The service only
//! runs against an already-installed managed runtime directory.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use deepagent_core::error::{CoreError, Result};
use serde_json::Value;

use crate::dto::{VisionRecognizeRequestDto, VisionRecognizeResultDto};
use crate::runtime_service::RuntimeService;

const FLORENCE_MODEL_ID: &str = "vision-florence-2-base-ft";
const FLORENCE_CAPABILITY: &str = "vision-image-to-text";

const FLORENCE_SCRIPT: &str = r#"
import json
import sys

model_dir = sys.argv[1]
image_path = sys.argv[2]
task = sys.argv[3] if len(sys.argv) > 3 and sys.argv[3] else "auto"

try:
    import torch
    from PIL import Image
    from transformers import AutoModelForCausalLM, AutoProcessor
except Exception as exc:
    print(json.dumps({"error": "missing python vision dependencies: " + str(exc)}, ensure_ascii=False))
    sys.exit(2)

device = "cuda:0" if torch.cuda.is_available() else "cpu"
dtype = torch.float16 if torch.cuda.is_available() else torch.float32

try:
    processor = AutoProcessor.from_pretrained(
        model_dir,
        trust_remote_code=True,
        local_files_only=True,
    )
    model = AutoModelForCausalLM.from_pretrained(
        model_dir,
        trust_remote_code=True,
        local_files_only=True,
        torch_dtype=dtype,
    ).to(device)
    model.eval()
    image = Image.open(image_path).convert("RGB")

    def run_task(task_prompt):
        inputs = processor(text=task_prompt, images=image, return_tensors="pt")
        inputs = {key: value.to(device) for key, value in inputs.items()}
        if "pixel_values" in inputs:
            inputs["pixel_values"] = inputs["pixel_values"].to(dtype)
        with torch.no_grad():
            generated_ids = model.generate(
                input_ids=inputs.get("input_ids"),
                pixel_values=inputs.get("pixel_values"),
                max_new_tokens=1024,
                num_beams=3,
                do_sample=False,
            )
        generated_text = processor.batch_decode(generated_ids, skip_special_tokens=False)[0]
        parsed = processor.post_process_generation(
            generated_text,
            task=task_prompt,
            image_size=(image.width, image.height),
        )
        return parsed

    tasks = ["<MORE_DETAILED_CAPTION>", "<OCR>"] if task == "auto" else [task]
    results = {}
    parts = []
    for current in tasks:
        parsed = run_task(current)
        results[current] = parsed
        if isinstance(parsed, dict):
            value = parsed.get(current) or next(iter(parsed.values()), "")
        else:
            value = parsed
        value = str(value).strip()
        if value:
            label = "视觉描述" if current == "<MORE_DETAILED_CAPTION>" else "图片文字"
            parts.append(label + "：" + value)

    print(json.dumps({"text": "\n".join(parts), "results": results}, ensure_ascii=False))
except Exception as exc:
    print(json.dumps({"error": "florence inference failed: " + str(exc)}, ensure_ascii=False))
    sys.exit(1)
"#;

#[derive(Clone)]
pub struct VisionService {
    runtime: Arc<RuntimeService>,
}

impl VisionService {
    pub fn new(runtime: Arc<RuntimeService>) -> Self {
        Self { runtime }
    }

    pub fn recognize_image(
        &self,
        request: VisionRecognizeRequestDto,
    ) -> Result<VisionRecognizeResultDto> {
        let image_path = PathBuf::from(&request.image_path);
        if !image_path.is_file() {
            return Err(CoreError::Other(format!(
                "image file not found: {}",
                image_path.display()
            )));
        }

        let model_dir = self.runtime.resolve(FLORENCE_CAPABILITY).ok_or_else(|| {
            CoreError::Other(format!(
                "vision model not installed - download runtime '{FLORENCE_MODEL_ID}' first"
            ))
        })?;
        ensure_florence_model_dir(&model_dir)?;

        let python = find_python().ok_or_else(|| {
            CoreError::Other(
                "python runtime not found - set DEEPAGENT_VISION_PYTHON or install Python with torch, pillow and transformers".to_string(),
            )
        })?;
        let prompt = request.prompt.unwrap_or_else(|| "auto".to_string());
        let output = Command::new(&python)
            .arg("-c")
            .arg(FLORENCE_SCRIPT)
            .arg(&model_dir)
            .arg(&image_path)
            .arg(prompt)
            .env("PYTHONIOENCODING", "utf-8")
            .output()
            .map_err(|e| CoreError::Other(format!("run Florence python: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let value: Value = serde_json::from_str(&stdout).map_err(|e| {
            CoreError::Other(format!(
                "parse Florence output: {e}; stdout: {stdout}; stderr: {stderr}"
            ))
        })?;
        if !output.status.success() {
            let message = value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("Florence inference failed");
            return Err(CoreError::Other(format!("{message}; stderr: {stderr}")));
        }
        let text = value
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            return Err(CoreError::Other(
                "Florence inference returned empty text".to_string(),
            ));
        }

        Ok(VisionRecognizeResultDto {
            model_id: FLORENCE_MODEL_ID.to_string(),
            model_path: model_dir.to_string_lossy().into_owned(),
            text,
            raw_json: stdout,
        })
    }
}

fn ensure_florence_model_dir(model_dir: &Path) -> Result<()> {
    for file in [
        "config.json",
        "configuration_florence2.py",
        "model.safetensors",
        "modeling_florence2.py",
        "preprocessor_config.json",
        "processing_florence2.py",
        "tokenizer.json",
        "tokenizer_config.json",
        "vocab.json",
    ] {
        let path = model_dir.join(file);
        if !path.is_file() {
            return Err(CoreError::Other(format!(
                "Florence model file missing: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn find_python() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("DEEPAGENT_VISION_PYTHON") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    for candidate in ["python", "python3"] {
        if Command::new(candidate)
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
        {
            return Some(PathBuf::from(candidate));
        }
    }
    None
}
