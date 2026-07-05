from PIL import Image, ImageDraw, ImageFont
import os

# Settings
output_dir = "numbered_images"
os.makedirs(output_dir, exist_ok=True)

width, height = 256, 256   # image size
font_path = "Maplestory OTF Bold.otf"    # Source: https://maplestory.nexon.com/Media/Font
font_size = 110
font = ImageFont.truetype(font_path, font_size)

for num in range(101):  # 0 through 100
    # Create blank image (white background)
    img = Image.new("RGBA", (width, height), (255, 255, 255, 0))
    draw = ImageDraw.Draw(img)

    # Text to draw
    text = str(num)
    text_width, text_height = draw.textsize(text, font=font)

    # Center horizontally, place above image (top margin)
    x = (width - text_width) // 2
    y = (height - text_height) // 2

    draw.text((x, y), text, font=font, fill="white")

    # Save image
    img.save(os.path.join(output_dir, f"{num}.png"))
