# Code Mode

The minimal pattern for giving an LLM a code-execution tool backed by Isola:

1. Call the model with a custom tool named `run_code`.
2. The model returns Python source as a tool call.
3. Execute that code inside an Isola sandbox.
4. Send the sandbox result back as the tool output.

The generated code can use normal Python, including `asyncio.gather(...)` for
parallel host calls.

The example below uses a morning-brief workflow: generate a weather report for
someone deciding what to wear and whether they need any warnings before heading
out.

## Example

```python
import asyncio
import json
import random

from openai import AsyncOpenAI  # or any LLM client
from isola import build_template
from textwrap import indent

client = AsyncOpenAI()
MODEL = "gpt-5.4"  # swap for any model that supports custom tools


async def get_current_weather(payload: dict[str, object]) -> object:
    await asyncio.sleep(0.05)
    city = str(payload["city"])
    conditions = random.choice(["sunny", "rain", "cloudy"])
    return {
        "city": city,
        "temperature_c": random.randint(18, 31),
        "conditions": conditions,
        "uv_index": random.randint(0, 11),
    }


async def get_forecast(payload: dict[str, object]) -> object:
    await asyncio.sleep(0.05)
    city = str(payload["city"])
    days = ["Mon", "Tue", "Wed"]
    return [
        {
            "city": city,
            "day": day,
            "high_c": random.randint(20, 32),
            "low_c": random.randint(14, 24),
            "conditions": random.choice(["sunny", "rain", "cloudy"]),
        }
        for day in days
    ]


async def get_air_quality(payload: dict[str, object]) -> object:
    await asyncio.sleep(0.05)
    city = str(payload["city"])
    aqi = random.randint(20, 140)
    return {
        "city": city,
        "aqi": aqi,
        "category": (
            "good" if aqi < 50 else "moderate" if aqi < 100 else "unhealthy"
        ),
    }


async def get_advisory(payload: dict[str, object]) -> object:
    await asyncio.sleep(0.05)
    kind = str(payload["kind"])
    messages = {
        "uv": "Use sunscreen and limit midday sun exposure.",
        "rain": "Carry an umbrella for the rainy day in the forecast.",
        "air_quality": "Sensitive groups should reduce prolonged outdoor activity.",
    }
    return {
        "kind": kind,
        "message": messages[kind],
    }


PRELUDE = """
import asyncio

from typing import Literal, TypedDict, cast

from sandbox.asyncio import hostcall


class CurrentWeather(TypedDict):
    city: str  # city name
    temperature_c: int  # current temperature in Celsius
    conditions: Literal["sunny", "rain", "cloudy"]  # current main conditions
    uv_index: int  # UV index on the standard 0-11+ scale; 7+ is high


class ForecastDay(TypedDict):
    city: str  # city name
    day: str  # short day label
    high_c: int  # forecast high in Celsius
    low_c: int  # forecast low in Celsius
    conditions: Literal["sunny", "rain", "cloudy"]  # expected main conditions


class AirQuality(TypedDict):
    city: str  # city name
    aqi: int  # air quality index score; 0-50 good, 51-100 moderate, 101+ unhealthy
    category: Literal["good", "moderate", "unhealthy"]  # normalized AQI bucket


class Advisory(TypedDict):
    kind: Literal["uv", "rain", "air_quality"]  # advisory category
    message: str  # suggested action for the user


async def get_current_weather(city: str) -> CurrentWeather:
    return cast(CurrentWeather, await hostcall("get_current_weather", {"city": city}))


async def get_forecast(city: str) -> list[ForecastDay]:
    return cast(list[ForecastDay], await hostcall("get_forecast", {"city": city}))


async def get_air_quality(city: str) -> AirQuality:
    return cast(AirQuality, await hostcall("get_air_quality", {"city": city}))


async def get_advisory(kind: str) -> Advisory:
    return cast(Advisory, await hostcall("get_advisory", {"kind": kind}))
""".strip()


def make_prelude_stub(prelude: str) -> str:
    lines: list[str] = []
    skipping_function_body = False

    for line in prelude.splitlines():
        stripped = line.strip()

        if stripped.startswith("import ") or stripped.startswith("from "):
            continue

        if line.startswith("async def "):
            lines.append(f"{line} ...")
            skipping_function_body = True
            continue

        if skipping_function_body:
            if stripped == "":
                lines.append("")
                skipping_function_body = False
            continue

        lines.append(line)

    return "\n".join(lines).strip()


PRELUDE_STUB = make_prelude_stub(PRELUDE)


TOOL_DESCRIPTION = f"""
Write Python only and return only source code.
`main()` must have the shape `async def main() -> object:` and must return a plain-text weather report as a string.

The sandbox prelude already provides `asyncio` and:

{indent(PRELUDE_STUB, "    ")}

Use this exact shape:

    async def main() -> object:
        current, forecast, air_quality = await asyncio.gather(
            get_current_weather("Tokyo"),
            get_forecast("Tokyo"),
            get_air_quality("Tokyo"),
        )

        advisory_tasks = []
        if current["uv_index"] >= 7:
            advisory_tasks.append(get_advisory("uv"))
        if any(day["conditions"] == "rain" for day in forecast):
            advisory_tasks.append(get_advisory("rain"))
        if air_quality["aqi"] >= 80:
            advisory_tasks.append(get_advisory("air_quality"))

        advisories = await asyncio.gather(*advisory_tasks) if advisory_tasks else []

        return "..."

""".strip()

async def run_code(template, code: str) -> object:
    async with template.create(
        hostcalls={
            "get_current_weather": get_current_weather,
            "get_forecast": get_forecast,
            "get_air_quality": get_air_quality,
            "get_advisory": get_advisory,
        }
    ) as sandbox:
        await sandbox.load_script(code)
        return await sandbox.run("main")

async def agent_loop(template, tools: list[dict[str, object]], prompt: str) -> str:
    response = await client.responses.create(
        model=MODEL,
        tools=tools,
        input=prompt,
    )

    while True:
        tool_outputs = []

        for item in response.output:
            if item.type != "custom_tool_call" or item.name != "run_code":
                continue

            result = await run_code(template, item.input)
            tool_outputs.append(
                {
                    "type": "custom_tool_call_output",
                    "call_id": item.call_id,
                    "output": json.dumps(result),
                }
            )

        if not tool_outputs:
            return response.output_text

        response = await client.responses.create(
            model=MODEL,
            previous_response_id=response.id,
            tools=tools,
            input=tool_outputs,
        )

async def main() -> None:
    template = await build_template("python", prelude=PRELUDE)

    tools = [
        {
            "type": "custom",
            "name": "run_code",
            "description": TOOL_DESCRIPTION,
        }
    ]

    output = await agent_loop(
        template,
        tools,
        prompt=(
            "Create a compact morning weather report for someone leaving for work in Tokyo. "
            "Look up the current weather, the short forecast, and the air quality. "
            "Only include advisories when they are actually relevant: "
            "include a UV advisory when the sun exposure is high, "
            "include a rain advisory when the forecast shows rain, "
            "and include an air quality advisory when air quality is poor enough to matter. "
            "Return a concise plain-text report, not JSON."
        ),
    )
    print(output)

asyncio.run(main())

```

## Example Run

During development, it is useful to inspect three things:

- `call_code`: the Python the model generated for `run_code`
- `json_response`: the JSON-encoded value returned by `run_code`
- `final_result`: the final plain-text answer after the model sees that tool result

The example above only prints `final_result`, but the other two values are
useful when you are debugging or tuning the tool description.

For one sample run, the generated code looked like this:

```python
async def main() -> object:
    current, forecast, air_quality = await asyncio.gather(
        get_current_weather("Tokyo"),
        get_forecast("Tokyo"),
        get_air_quality("Tokyo"),
    )

    advisories = []

    if current["uv_index"] >= 7:
        advisories.append(await get_advisory("uv"))

    if any(day["conditions"] == "rain" for day in forecast):
        advisories.append(await get_advisory("rain"))

    if air_quality["aqi"] >= 101 or air_quality["category"] == "unhealthy":
        advisories.append(await get_advisory("air_quality"))

    ...
```

And the final result was a short report:

```text
Tokyo morning report: 25°C and sunny.
Forecast: Mon: cloudy, 29°/16°; Tue: sunny, 30°/14°; Wed: rain, 24°/22°.
Air quality: AQI 77 (moderate).
Advisories: Carry an umbrella for the rainy day in the forecast.
```

That is the whole pattern:

- The model decides when to call `run_code`.
- Isola executes the generated code in a sandbox.
- The generated code uses typed prelude helpers backed by `hostcall(...)`.
- `asyncio.gather(...)` handles parallel fetches inside the sandbox.
- The tool result stays structured internally while the final user-facing
  answer is a polished text report.
