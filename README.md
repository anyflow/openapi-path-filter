# openapi-path-filter

## Introduction

A Rust-based Istio WASM filter that injects a Prometheus label representing the request path, based on a path defined in the OpenAPI spec.

## Advantages

- URL matching 최적화: `matchit` lib 사용(benchmark 결과 가장 빠르다고. [참조](https://github.com/ibraheemdev/matchit?tab=readme-ov-file#benchmarks))

## Tasks

- ✅ 정상 등록 및 실제 동작 검증
- ✅ 동적 wasm 모듈 로딩 테스트
- ✅ 단위 테스트 보강
- ✅ build 자동화: cargo-make 사용
- ✅ cargo/docker image version 자동 동기화(`${CARGO_MAKE_CRATE_VERSION}` in `Makefile.toml`)
- ✅ image optimization (`wasm-opt` 도입)
- ✅ Fast fail, optimization 포함 build step 정렬
- ✅ Single thread 용으로 전환(`Rc<T>` 사용). proxy WASM은 single thread로 동작하므로.
- 🚧 `proxy-wasm-test-framework = { git = "https://github.com/proxy-wasm/test-framework" }` 사용하여 테스트 가능하도록: runtime 검증용. 이게 되기 전까지는 [runtime 테스트 방법 in istio](#runtime-테스트-방법-in-istio) 로 검증해야.
- 💧 **LRU 캐시 도입**: `lru` lib이 적절하지만 read에 조차 lock을 써야하기에 오히려 성능 저하 크고 복잡도가 증가. `cache` branch 참조.

## Getting started

```shell
# Rust 설치. 참고로 macOS에서 brew로 설치하면 정상 compile안됨. 따라서 Rust 공식 설치 Path를 따라야.
> curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# cargo-make 설치 (빌드 도구. Makefile.toml 참고)
> cargo install cargo-make

# wasm-opt (bynaryen) 설치 (macOS의 경우. 타 OS의 경우 별도 방법 필요. 설치 안될 경우 Makefile.toml의 optimize-wasm task 제거로 본 step skip 가능
> brew install binaryen

# test -> rust build -> image optimization -> docker build -> docker push
> cargo make clean-all

# 정상 등록 여부 확인
> curl -X GET https://docker-registry.anyflow.net/v2/openapi-path-filter/manifests/latest \
  -H "Accept: application/vnd.oci.image.manifest.v1+json"
```

## runtime 테스트 방법 in istio

```shell

# 대상 pod wasm log level을 debug로 변경
> istioctl pc log -n <namespace name> <pod name> --level wasm:debug

# openapi-path-filter 만 logging
> k logs -n <namespace name> <pod name> -f | grep -F '[opf]'

# resource/telemetry.yaml 적용: x-openapi-path header, method를 각각 request_path, request_method란 metric label로 넣기 위함
> kubectl apply -f telemetry.yaml

# resources/wasmplugin.yaml 적용: 정상 loading 여부 확인을 위한 log 확인. e.g. "[opf] Router configured successfully"
> kubectl apply -f wasmplugin.yaml

# curl로 호출 후 log에 matching 여부 log가 나오는지 확인. e.g. "[opf] Path '/dockebi/v1/stuff' matched with value: /dockebi/v1/stuff"
> curl https://api.anyflow.net/dockebi/v1/stuff
```

## 참고

### `[opf]` log prefix에 관하여

log grep 용. `openapi-path-filter` 만 갖고는 전체 `grep` 불가하기 때문. 아래 첫 번째처럼 `openapi-path-filter` 가 자동으로 붙는 경우도 있지만 두 번째처럼 안붙는 경우도 있기 때문.

- `2025-02-23T20:30:59.970615Z     debug   envoy wasm external/envoy/source/extensions/common/wasm/context.cc:1192 wasm log cluster.openapi-path-filter: [opf] Creating HTTP context       thread=29`
- `2025-02-23T20:28:39.632084Z     info    envoy wasm external/envoy/source/extensions/common/wasm/context.cc:1195 wasm log: [opf] Router configured successfully  thread=20`


### wasm unloading 확인 방법에 관하여

`kubectl delete -f wasmplugin.yaml` 을 하더라도 그 즉시 wasm이 Envoy에서 삭제되는 것이 아닌 30s ~ 60s이 지난 후에 삭제되는 듯. 아래와 같은 로그로 확인 가능. 새로운 wasm 동작 확인 필요 시 기존 wasm 제거 후 아래 메시지 확인 후 새 wasm 로드 필요.

- `2025-02-23T19:35:58.014282Z     info    envoy wasm external/envoy/source/extensions/common/wasm/context.cc:1195 wasm log: openapi-path-filter terminated        thread=20`

- `2025-02-23T05:51:26.936732Z     debug   envoy init external/envoy/source/common/init/target_impl.cc:68  shared target FilterConfigSubscription init extenstions.istio.io/wasmplugin/cluster.openapi-path-filter destroyedthread=20`


### docker-registry 명령어

```shell
# image catalog 얻기
❯ curl -X GET https://docker-registry.anyflow.net/v2/_catalog
{"repositories":["api-signature-filter","doc-publisher","dockebi","docserver","manifest-generator","openapi-path-filter","staffonly"]}


# tag 목록 얻기
❯ curl -X GET https://docker-registry.anyflow.net/v2/openapi-path-filter/tags/list
{"name":"openapi-path-filter","tags":["1.0.0","latest","0.1.0"]}

# image digest 얻기
❯ curl -X GET https://docker-registry.anyflow.net/v2/openapi-path-filter/manifests/latest \
  -H "Accept: application/vnd.docker.distribution.manifest.v2+json" \
  -v 2>&1 | grep docker-content-digest | awk '{print ($3)}'
sha256:956f9ebd2cd44b60e82d5cfc0e2b9c12ca04e61532d8e024a4cc712dea011277

# image 삭제하기 (REGISTRY_STORAGE_DELETE_ENABLED=true 설정 필요 in docker-registry)
curl -X DELETE https://docker-registry.anyflow.net/v2/openapi-path-filter/manifests/<image digest>
```
