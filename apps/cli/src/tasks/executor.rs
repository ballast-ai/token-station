//! Exact package resolution, descriptor authorization and bounded host transport.
use super::{Binding, ComponentPin, Provider, digest, read_bounded};
use south_component_conformance::{
    PreparedTaskV2, SubmitOutcomeV2, TaskComponentV2, sandbox_task_v2::SandboxedTaskComponentV2,
    task_v2_json,
};
use south_contracts::{HostMintedValuesV1, TaskObservationV2, TaskRenderContextV2};
use south_provider_api::HostExpectationsV1;
use south_provider_runtime::{ComponentRuntimeV1, LoadedComponentV1, NoSecretsV1, RuntimeLimitsV1};
use std::io::Read;
use std::path::{Path, PathBuf};
use task_protocol::{
    Auth, HttpMethod, HttpRequestDescriptor, HttpResponseParts, ProviderConfig, ProviderEndpoint,
    SecretRef,
};

pub struct Executor {
    component: SandboxedTaskComponentV2,
    config: ProviderConfig,
}
fn candidates(root: &Path, depth: u8, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let metadata =
        std::fs::symlink_metadata(root).map_err(|_| "task component root is unavailable")?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("task component root must be a real directory".into());
    }
    if root.join("manifest.json").exists() {
        out.push(root.to_owned());
        return Ok(());
    }
    if depth == 0 {
        return Err("task package nesting exceeds limit".into());
    }
    for entry in std::fs::read_dir(root).map_err(|_| "cannot scan task components")? {
        let entry = entry.map_err(|_| "cannot scan task components")?;
        let kind = entry
            .file_type()
            .map_err(|_| "cannot inspect task component")?;
        if kind.is_symlink() {
            return Err("task package symlinks are forbidden".into());
        }
        if kind.is_dir() {
            candidates(&entry.path(), depth - 1, out)?;
        }
        if out.len() > 128 {
            return Err("too many task packages".into());
        }
    }
    Ok(())
}
impl Executor {
    pub fn load(root: &Path, provider: &Provider) -> Result<Self, String> {
        let dialect = match provider.dialect.as_str() {
            "bailian_video" => "bailian",
            "minimax_video" => "minimax",
            _ => return Err("unsupported task dialect".into()),
        };
        let pin = &provider.pin;
        if pin.world != "task-adapter-v2" {
            return Err("unsupported task world".into());
        }
        let mut paths = Vec::new();
        candidates(root, 6, &mut paths)?;
        let mut package = None;
        for dir in paths {
            for file in ["manifest.json", "component.wasm"] {
                let meta = std::fs::symlink_metadata(dir.join(file))
                    .map_err(|_| "task package is incomplete")?;
                if meta.file_type().is_symlink() || !meta.is_file() {
                    return Err("task package files must be regular files".into());
                }
            }
            let manifest = read_bounded(&dir.join("manifest.json"), 256 * 1024)?;
            if digest(&manifest) != pin.manifest_sha256 {
                continue;
            }
            let wasm = read_bounded(&dir.join("component.wasm"), 64 * 1024 * 1024)?;
            if digest(&wasm) != pin.wasm_sha256 {
                return Err("task component digest mismatch".into());
            }
            if package.is_some() {
                return Err("duplicate exact task component package".into());
            }
            package = Some((manifest, wasm));
        }
        let (manifest, wasm) =
            package.ok_or("the exact saved task component package is not present")?;
        let runtime = ComponentRuntimeV1::new(RuntimeLimitsV1::default())
            .map_err(|_| "cannot initialize task runtime")?;
        let expected = HostExpectationsV1 {
            ir_schema_id: "token-station-protocol@0.3.0/v0.2.0".into(),
            kernel_version: "0.2.0".into(),
            kernel_revision: "72458e3a11fe157f9ac04818c44b62a3dd2cb09c".into(),
            south_runtime: "0.31.0".into(),
        };
        let loaded = LoadedComponentV1::load_embedded(
            &runtime,
            std::str::from_utf8(&manifest).map_err(|_| "invalid task manifest")?,
            &wasm,
            &expected,
            NoSecretsV1,
        )
        .map_err(|_| "task component package is incompatible or invalid")?;
        let metadata = loaded.manifest();
        let actual = ComponentPin {
            world: metadata.api_version.clone(),
            name: metadata.name.clone(),
            version: metadata.version.clone(),
            manifest_sha256: digest(&manifest),
            wasm_sha256: digest(&wasm),
        };
        if actual != *pin
            || !metadata.providers.iter().any(|p| p == dialect)
            || !metadata
                .permissions
                .secrets
                .iter()
                .any(|s| s == "provider_api_key")
            || !metadata.auth_arms.contains("bearer")
        {
            return Err("task package identity or credential capability mismatch".into());
        }
        let mut config = ProviderConfig::new(
            dialect,
            ProviderEndpoint::try_new(&provider.endpoint).map_err(|_| "invalid task endpoint")?,
        );
        config.auth = Some(SecretRef::new("provider_api_key"));
        config.extensions.insert(
            "upstream_model_family".into(),
            serde_json::json!(provider.model),
        );
        let component = SandboxedTaskComponentV2::new(loaded).map_err(|_| "task world mismatch")?;
        Ok(Self { component, config })
    }
    fn capability(&self, name: &str) -> Result<(), String> {
        if self
            .component
            .inner()
            .manifest()
            .capabilities
            .contains(name)
        {
            Ok(())
        } else {
            Err("task operation was not declared".into())
        }
    }
    fn authorize(&self, descriptor: &HttpRequestDescriptor) -> Result<(), String> {
        self.config
            .authorize(descriptor)
            .map_err(|_| "task descriptor authorization failed")?;
        if descriptor.auth != Some(Auth::bearer(SecretRef::new("provider_api_key"))) {
            return Err("unsupported task authentication".into());
        }
        Ok(())
    }
    pub fn prepare(&self, request: &serde_json::Value, id: &str) -> Result<PreparedTaskV2, String> {
        self.capability("submit")?;
        let value = self
            .component
            .build_submit_request(
                &self.config,
                request,
                &HostMintedValuesV1::new(id, None).map_err(|_| "invalid task identity")?,
            )
            .map_err(|_| "task request was rejected by component")?;
        self.authorize(&value.descriptor)?;
        Ok(value)
    }
    pub async fn submit(
        &self,
        descriptor: &HttpRequestDescriptor,
        secret: &str,
    ) -> Result<SubmitOutcomeV2, String> {
        self.capability("submit")?;
        let response = self.execute(descriptor, secret).await?;
        self.component
            .parse_submit_response(&response)
            .map_err(|_| "task submit response is invalid".into())
    }
    pub async fn observe(
        &self,
        binding: &Binding,
        id: &str,
        secret: &str,
    ) -> Result<TaskObservationV2, String> {
        self.capability("observe")?;
        let locator = task_v2_json::parse_locator_json(&binding.locator.to_string())
            .map_err(|_| "invalid saved task locator")?;
        let descriptor = self
            .component
            .build_observe_request(&self.config, &binding.provider.model, id, &locator)
            .map_err(|_| "task query could not be prepared")?;
        let response = self.execute(&descriptor, secret).await?;
        self.component
            .parse_observation(&response)
            .map_err(|_| "task observation is invalid".into())
    }
    pub async fn render(
        &self,
        binding: &Binding,
        id: &str,
        upstream_id: &str,
        observation: &TaskObservationV2,
        secret: &str,
    ) -> Result<serde_json::Value, String> {
        self.capability("render")?;
        let locator = task_v2_json::parse_locator_json(&binding.locator.to_string())
            .map_err(|_| "invalid saved task locator")?;
        let descriptor = self
            .component
            .build_artifact_request(&self.config, &locator, observation)
            .map_err(|_| "task artifact request is invalid")?;
        let fetched = if let Some(descriptor) = descriptor {
            self.capability("artifact_fetch")?;
            Some(self.execute(&descriptor, secret).await?)
        } else {
            None
        };
        let created = i64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "invalid system clock")?
                .as_secs(),
        )
        .map_err(|_| "invalid system clock")?;
        let context = TaskRenderContextV2::new(
            id,
            created,
            &binding.provider.model,
            &self.config.provider,
            Some(upstream_id),
        )
        .map_err(|_| "invalid task rendering context")?;
        self.component
            .render_success(observation, fetched.as_ref(), &context)
            .map_err(|_| "task rendering failed".into())
    }
    async fn execute(
        &self,
        descriptor: &HttpRequestDescriptor,
        secret: &str,
    ) -> Result<HttpResponseParts, String> {
        self.authorize(descriptor)?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "cannot initialize task transport")?;
        let mut request = match descriptor.method {
            HttpMethod::Get => {
                if descriptor.body.is_some() {
                    return Err("task GET must not have a body".into());
                }
                client.get(&descriptor.url)
            }
            HttpMethod::Post => client.post(&descriptor.url).body(
                descriptor
                    .body
                    .as_ref()
                    .ok_or("task POST requires a body")?
                    .to_string(),
            ),
        }
        .bearer_auth(secret);
        for (name, value) in descriptor.headers.iter() {
            request = request.header(name, value);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| "task transport outcome unavailable")?;
        let status = response.status().as_u16();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "task response read failed")?
        {
            if chunk.len() > (4 * 1024 * 1024usize).saturating_sub(bytes.len()) {
                return Err("task response exceeds limit".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(HttpResponseParts {
            status,
            body: String::from_utf8(bytes).map_err(|_| "task response is not UTF-8")?,
            headers: std::collections::BTreeMap::default(),
            extensions: std::collections::BTreeMap::default(),
        })
    }
}

pub fn fetch(url: &str, output: &Path) -> Result<(), String> {
    if output.exists() {
        return Err("artifact output already exists".into());
    }
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .max_redirects(0)
        .build()
        .new_agent();
    let mut response = agent
        .get(url)
        .call()
        .map_err(|_| "artifact download failed")?;
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let temp = parent.join(format!(".task-artifact-{}", super::random_id()?));
    token_station_private_fs::create_private_file(&temp, b"")
        .map_err(|_| "cannot create artifact temporary file")?;
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&temp)
            .map_err(|_| "cannot open artifact file")?;
        let n = std::io::copy(
            &mut response.body_mut().as_reader().take(64 * 1024 * 1024 + 1),
            &mut file,
        )
        .map_err(|_| "artifact read failed")?;
        if n > 64 * 1024 * 1024 {
            return Err("artifact exceeds size limit".to_owned());
        }
        file.sync_all().map_err(|_| "cannot sync artifact")?;
        std::fs::hard_link(&temp, output)
            .map_err(|_| "cannot publish artifact without replacing output")?;
        Ok(())
    })();
    let _ = std::fs::remove_file(temp);
    result
}
