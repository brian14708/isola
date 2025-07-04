try:
    from _promptkit_serde import json_dumps, json_loads, yaml_dumps, yaml_loads
except ImportError:
    import json
    from typing import Any

    import yaml

    json_dumps = json.dumps
    json_loads = json.loads

    def yaml_dumps(obj: Any) -> str:
        return yaml.dump(obj)

    def yaml_loads(s: str) -> Any:
        return yaml.safe_load(s)


__all__ = ["json_dumps", "json_loads", "yaml_dumps", "yaml_loads"]
