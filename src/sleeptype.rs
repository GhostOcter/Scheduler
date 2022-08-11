#[cfg(feature = "serde")]
use serde::{
    de::{EnumAccess, Visitor},
    Deserialize, Serialize,
};
#[cfg(feature = "spin_sleep")]
use spin_sleep::SpinSleeper;
#[cfg(all(feature = "spin_sleep", feature = "serde"))]
use {
    serde::{de::VariantAccess, ser::SerializeStructVariant},
    spin_sleep::SpinStrategy,
};
// You need to know that the ...
#[derive(PartialEq, Eq, Clone, Debug, Default)]
pub enum SleepType {
    #[default]
    // Used when you need accuracy to the second. In this case, the scheduler uses std::thread::sleep() which has no cost to your program or computer.
    Native,
    // Accurate to the millisecond => Use spin sleep which require more ressoruces to work
    #[cfg(feature = "spin_sleep")]
    SpinSleep(SpinSleeper),
}
#[cfg(feature = "serde")]
impl Serialize for SleepType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &self {
            Self::Native => serializer.serialize_unit_variant("SleepType", 0, "Native"),
            #[cfg(feature = "spin_sleep")]
            Self::SpinSleep(spin_sleeper) => {
                let mut sv = serializer.serialize_struct_variant("SleepType", 1, "SpinSleep", 2)?;
                sv.serialize_field(
                    "native_accuracy_ns",
                    &spin_sleeper.clone().native_accuracy_ns(),
                )?;
                sv.serialize_field(
                    "spin_strategy",
                    if spin_sleeper.spin_strategy() == SpinStrategy::YieldThread {
                        &0
                    } else {
                        &1
                    },
                )?;
                sv.end()
            }
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> Deserialize<'de> for SleepType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SleepVisitor;
        impl<'de> Visitor<'de> for SleepVisitor {
            type Value = SleepType;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("Expecting serialized SleepType enum")
            }
            #[cfg(not(feature = "spin_sleep"))]
            fn visit_enum<A>(self, _: A) -> Result<Self::Value, A::Error>
            where
                A: EnumAccess<'de>,
            {
                Ok(SleepType::Native)
            }
            #[cfg(feature = "spin_sleep")]
            fn visit_enum<A>(self, data: A) -> Result<Self::Value, A::Error>
            where
                A: EnumAccess<'de>,
            {
                let variant = data.variant::<String>()?;
                if variant.0 == "Native" {
                    Ok(SleepType::Native)
                } else {
                    Ok(variant
                        .1
                        .struct_variant(&["native_accuracy_ns", "spin_strategy"], Self)?)
                }
            }

            #[cfg(feature = "spin_sleep")]
            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                Ok(SleepType::SpinSleep(
                    SpinSleeper::new(
                        map.next_entry::<String, u32>()?
                            .expect("Native accuracy field")
                            .1,
                    )
                    .with_spin_strategy(
                        if map
                            .next_entry::<String, u8>()?
                            .expect("Spin strategy field")
                            .1
                            == 0
                        {
                            SpinStrategy::YieldThread
                        } else {
                            SpinStrategy::SpinLoopHint
                        },
                    ),
                ))
            }
        }
        deserializer.deserialize_enum("SleepType", &["Native", "SpinSleep"], SleepVisitor)
    }
}
