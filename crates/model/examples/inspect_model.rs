//! Inspect a bounded U3D stream and its first transformed triangle.
use pdfcraft_model::three_d::Scene;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).ok_or("provide a U3D path")?;
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > 64 * 1024 * 1024 {
        return Err("U3D stream exceeds 64 MiB".into());
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    file.take(64 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    let scene = Scene::decode_u3d(&bytes)?;
    println!("meshes={} instances={} meters_per_unit={:?}", scene.meshes.len(), scene.instances.len(), scene.meters_per_unit);
    println!("bounds={:?}", scene.bounds()?);
    for mesh in &scene.meshes {
        println!(
            "mesh={:?} positions={} triangles={} normals={} encoding={:?}",
            mesh.name,
            mesh.positions.len(),
            mesh.triangles.len(),
            mesh.normals.len(),
            scene.normal_encoding
        );
        let mut minimum_cosine = 1.0_f64;
        for (face, attributes) in mesh.triangles.iter().zip(&mesh.attributes) {
            let a = *mesh.positions.get(face[0] as usize).ok_or("invalid triangle position")?;
            let b = *mesh.positions.get(face[1] as usize).ok_or("invalid triangle position")?;
            let c = *mesh.positions.get(face[2] as usize).ok_or("invalid triangle position")?;
            let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
            let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
            let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
            let length = n.iter().map(|v| v * v).sum::<f64>().sqrt();
            if let Some(indices) = attributes.normals {
                for index in indices {
                    let normal = *mesh.normals.get(index as usize).ok_or("invalid normal index")?;
                    let size = normal.iter().map(|v| v * v).sum::<f64>().sqrt();
                    let dot = n.iter().zip(normal).map(|(a, b)| a * b).sum::<f64>() / (length * size);
                    minimum_cosine = minimum_cosine.min(dot);
                }
            }
        }
        println!("minimum_face_normal_cosine={minimum_cosine}");
        if let Some(indices) = mesh.attributes.first().and_then(|a| a.normals) {
            println!("first_normals={:?}", indices.map(|i| mesh.normals.get(i as usize)));
        }
    }
    if std::env::args_os().nth(2).as_deref() == Some(std::ffi::OsStr::new("--dump")) {
        for (mesh_index, mesh) in scene.meshes.iter().enumerate() {
            println!("M {mesh_index} {} {} {}", mesh.positions.len(), mesh.triangles.len(), mesh.normals.len());
            for (index, p) in mesh.positions.iter().enumerate() {
                println!("P {index} {} {} {}", p[0], p[1], p[2]);
            }
            for (index, n) in mesh.normals.iter().enumerate() {
                println!("N {index} {} {} {}", n[0], n[1], n[2]);
            }
            for (kind, pool) in [("D", &mesh.diffuse), ("S", &mesh.specular), ("T", &mesh.texture_coordinates)] {
                for (index, value) in pool.iter().enumerate() {
                    println!("{kind} {index} {} {} {} {}", value[0], value[1], value[2], value[3]);
                }
            }
            for (index, point) in mesh.points.iter().enumerate() {
                let normal = mesh.point_attributes.get(index).and_then(|a| a.normals).unwrap_or([0]);
                println!("O {index} {point} {}", normal[0]);
                if let Some(color) = mesh.point_attributes.get(index).and_then(|a| a.diffuse) {
                    println!("OD {index} {}", color[0]);
                }
            }
            for (index, line) in mesh.lines.iter().enumerate() {
                let normal = mesh.line_attributes.get(index).and_then(|a| a.normals).unwrap_or([0; 2]);
                println!("L {index} {} {} {} {}", line[0], line[1], normal[0], normal[1]);
                if let Some(color) = mesh.line_attributes.get(index).and_then(|a| a.diffuse) {
                    println!("LD {index} {} {}", color[0], color[1]);
                }
            }
            for (index, t) in mesh.triangles.iter().enumerate() {
                let n = mesh.attributes.get(index).and_then(|a| a.normals).unwrap_or([0; 3]);
                println!("F {index} {} {} {} {} {} {}", t[0], t[1], t[2], n[0], n[1], n[2]);
            }
        }
    }
    for (index, instance) in scene.instances.iter().take(20).enumerate() {
        println!("instance={index} name={:?} first_triangle={:?}", instance.name, scene.triangle(index, 0).ok());
    }
    Ok(())
}
