def pillow() -> None:
    from io import BytesIO

    from PIL import Image, ImageDraw, ImageFont

    img = Image.new("RGB", (200, 200), color="white")
    draw = ImageDraw.Draw(img)

    draw.rectangle((50, 50, 150, 150), outline="red", width=5)
    draw.ellipse((75, 75, 125, 125), outline="blue", width=5)

    # Load font or fallback
    try:
        font = ImageFont.truetype("arial.ttf", size=20)
    except OSError:
        font = ImageFont.load_default()

    text = "Hello PIL!"

    # Calculate text width/height
    if hasattr(draw, "textbbox"):
        # Pillow ≥10
        bbox = draw.textbbox((0, 0), text, font=font)
        w, _h = bbox[2] - bbox[0], bbox[3] - bbox[1]
    else:
        # Older Pillow
        w, _h = font.getsize(text)

    # Center text horizontally at y=10
    x = (img.width - w) // 2
    y = 10
    draw.text((x, y), text, font=font, fill="black")

    # Export to bytearrays
    buf_jpg = BytesIO()
    img.save(buf_jpg, format="JPEG")
    jpg_bytes = buf_jpg.getvalue()

    buf_png = BytesIO()
    img.save(buf_png, format="PNG")
    png_bytes = buf_png.getvalue()

    assert len(jpg_bytes) > 512
    assert len(png_bytes) > 512


def numpy() -> None:
    import numpy as np

    arr = np.random.default_rng(12345).integers(0, 100, size=(10, 10))
    mean = np.mean(arr)
    stddev = np.std(arr)
    mask = arr > mean
    filtered_values = arr[mask]
    assert arr.shape == (10, 10)
    assert mean >= 0
    assert mean < 100
    assert stddev >= 0
    assert len(filtered_values) > 0


def pydantic() -> None:
    import json

    from pydantic import BaseModel

    class Sanity(BaseModel):
        n: int

    s = Sanity(n=5)
    assert json.loads(s.json())["n"] == 5


def tzdata() -> None:
    from datetime import datetime, timedelta
    from zoneinfo import ZoneInfo

    now_tokyo = datetime.now(ZoneInfo("Asia/Tokyo"))
    utc_offset = now_tokyo.utcoffset()
    assert utc_offset == timedelta(hours=9), f"Asia/Tokyo offset != +9: {utc_offset}"
